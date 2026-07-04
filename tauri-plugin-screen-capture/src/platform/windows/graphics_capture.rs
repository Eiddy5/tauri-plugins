use std::{
    sync::mpsc as std_mpsc,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use async_trait::async_trait;
use tokio::{sync::watch, task::JoinHandle};
use windows::Win32::Graphics::Gdi::{
    BitBlt, CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC, GetMonitorInfoW,
    ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HGDIOBJ,
    HMONITOR, MONITORINFO, SRCCOPY,
};
use windows_capture::{
    capture::{CaptureControl, Context, GraphicsCaptureApiError, GraphicsCaptureApiHandler},
    dxgi_duplication_api::{DxgiDuplicationApi, DxgiDuplicationFormat},
    frame::Frame,
    graphics_capture_api::InternalCaptureControl,
    monitor::Monitor,
    settings::{
        ColorFormat, CursorCaptureSettings, DirtyRegionSettings, DrawBorderSettings,
        MinimumUpdateIntervalSettings, SecondaryWindowSettings, Settings,
    },
    window::Window,
};

use crate::{
    capture::{FrameConsumer, RunningCapture},
    error::Error,
    models::{
        CaptureErrorCode, CaptureSource, CaptureSourceKind, ListSourcesOptions, PixelFormat,
        StartCaptureOptions,
    },
    pipeline::frame::VideoFrame,
    sources::thumbnails::encode_png_base64,
    sources::{filter_sources, SourceFilterOptions},
    Result,
};

const THUMBNAIL_TIMEOUT: Duration = Duration::from_millis(600);
const THUMBNAIL_MAX_WIDTH: u32 = 420;
const WINDOW_THUMBNAIL_LIMIT: usize = 12;

type SharedFrameSlot = Arc<Mutex<Option<Result<VideoFrame>>>>;

pub fn list_sources(options: ListSourcesOptions) -> Result<Vec<CaptureSource>> {
    let mut sources = Vec::new();

    if options.kinds.contains(&CaptureSourceKind::Display) {
        sources.extend(enumerate_displays(options.include_thumbnails)?);
    }

    if options.kinds.contains(&CaptureSourceKind::Window) {
        sources.extend(enumerate_windows(options.include_thumbnails)?);
    }

    Ok(filter_sources(
        sources,
        SourceFilterOptions {
            current_pid: Some(std::process::id()),
            include_current_app: options.include_current_app,
            include_system_ui: options.include_system_ui,
            debug_raw_sources: options.debug_raw_sources,
        },
    ))
}

pub fn start_capture(
    options: StartCaptureOptions,
    consumer: Box<dyn FrameConsumer>,
) -> Result<Box<dyn RunningCapture>> {
    let fps = options.effective_fps();
    let paused = Arc::new(AtomicBool::new(false));
    let frame_slot = Arc::new(Mutex::new(None));
    let (finish_sender, _) = watch::channel(None);
    let control = start_windows_capture(&options, Arc::clone(&paused), Arc::clone(&frame_slot))?;
    let frame_interval = Duration::from_secs_f64(1.0 / f64::from(fps));
    let task_finish_sender = finish_sender.clone();

    let task = tokio::spawn(async move {
        let mut tick = tokio::time::interval(frame_interval);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            tick.tick().await;
            let next_frame = frame_slot.lock().ok().and_then(|mut slot| slot.take());
            match next_frame {
                Some(Ok(mut frame)) => {
                    frame.timestamp_ns = now_ns();
                    if let Err(error) = consumer.push_frame(frame).await {
                        eprintln!("[screen-capture] Windows frame publish failed: {error:?}");
                    }
                }
                Some(Err(error)) => {
                    eprintln!("[screen-capture] Windows frame capture failed: {error:?}");
                    let _ = task_finish_sender.send(Some(error.payload()));
                    break;
                }
                None => {}
            }
        }
    });

    Ok(Box::new(WindowsRunningCapture {
        control: Mutex::new(Some(control)),
        paused,
        task,
        finish_sender,
    }))
}

type WindowsCaptureControl = CaptureControl<WindowsCaptureHandler, String>;

struct WindowsRunningCapture {
    control: Mutex<Option<WindowsCaptureControl>>,
    paused: Arc<AtomicBool>,
    task: JoinHandle<()>,
    finish_sender: watch::Sender<Option<crate::models::CaptureErrorPayload>>,
}

#[async_trait]
impl RunningCapture for WindowsRunningCapture {
    async fn pause(&self) -> Result<()> {
        self.paused.store(true, Ordering::SeqCst);
        Ok(())
    }

    async fn resume(&self) -> Result<()> {
        self.paused.store(false, Ordering::SeqCst);
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        self.stop_capture()
    }

    fn finish_receiver(
        &self,
    ) -> Option<watch::Receiver<Option<crate::models::CaptureErrorPayload>>> {
        Some(self.finish_sender.subscribe())
    }
}

impl WindowsRunningCapture {
    fn stop_capture(&self) -> Result<()> {
        self.paused.store(true, Ordering::SeqCst);
        self.task.abort();

        let control = self
            .control
            .lock()
            .ok()
            .and_then(|mut control| control.take());
        if let Some(control) = control {
            control.stop().map_err(capture_control_error)?;
        }

        Ok(())
    }
}

impl Drop for WindowsRunningCapture {
    fn drop(&mut self) {
        self.paused.store(true, Ordering::SeqCst);
        self.task.abort();

        if let Ok(mut control) = self.control.lock() {
            if let Some(control) = control.take() {
                let _ = control.stop();
            }
        }
    }
}

struct WindowsCaptureHandler {
    frame_slot: SharedFrameSlot,
    paused: Arc<AtomicBool>,
    output_size: Option<(u32, u32)>,
}

struct WindowsCaptureFlags {
    frame_slot: SharedFrameSlot,
    paused: Arc<AtomicBool>,
    output_size: Option<(u32, u32)>,
}

impl GraphicsCaptureApiHandler for WindowsCaptureHandler {
    type Flags = WindowsCaptureFlags;
    type Error = String;

    fn new(ctx: Context<Self::Flags>) -> std::result::Result<Self, Self::Error> {
        Ok(Self {
            frame_slot: ctx.flags.frame_slot,
            paused: ctx.flags.paused,
            output_size: ctx.flags.output_size,
        })
    }

    fn on_frame_arrived(
        &mut self,
        frame: &mut Frame,
        _capture_control: InternalCaptureControl,
    ) -> std::result::Result<(), Self::Error> {
        if self.paused.load(Ordering::SeqCst) {
            return Ok(());
        }

        if self
            .frame_slot
            .lock()
            .is_ok_and(|slot| slot.as_ref().map(|frame| frame.is_ok()).unwrap_or(false))
        {
            return Ok(());
        }

        let video_frame = frame_to_video_frame(frame, self.output_size);
        if let Ok(mut slot) = self.frame_slot.lock() {
            if slot.is_none() {
                *slot = Some(video_frame);
            }
        }
        Ok(())
    }

    fn on_closed(&mut self) -> std::result::Result<(), Self::Error> {
        if let Ok(mut slot) = self.frame_slot.lock() {
            *slot = Some(Err(Error::new(
                CaptureErrorCode::SourceUnavailable,
                "Windows capture source was closed",
                true,
            )));
        }
        Ok(())
    }
}

struct ThumbnailCaptureHandler {
    sender: std_mpsc::Sender<Result<String>>,
}

struct ThumbnailCaptureFlags {
    sender: std_mpsc::Sender<Result<String>>,
}

impl GraphicsCaptureApiHandler for ThumbnailCaptureHandler {
    type Flags = ThumbnailCaptureFlags;
    type Error = String;

    fn new(ctx: Context<Self::Flags>) -> std::result::Result<Self, Self::Error> {
        Ok(Self {
            sender: ctx.flags.sender,
        })
    }

    fn on_frame_arrived(
        &mut self,
        frame: &mut Frame,
        capture_control: InternalCaptureControl,
    ) -> std::result::Result<(), Self::Error> {
        let thumbnail = frame_to_thumbnail(frame);
        let _ = self.sender.send(thumbnail);
        capture_control.stop();
        Ok(())
    }
}

fn start_windows_capture(
    options: &StartCaptureOptions,
    paused: Arc<AtomicBool>,
    frame_slot: SharedFrameSlot,
) -> Result<WindowsCaptureControl> {
    let flags = WindowsCaptureFlags {
        frame_slot,
        paused,
        output_size: requested_output_size(options),
    };
    match options.source_kind {
        CaptureSourceKind::Display => {
            let monitor = monitor_from_source_id(&options.source_id)?;
            let settings = Settings::new(
                monitor,
                cursor_settings(options),
                capture_draw_border_settings(),
                SecondaryWindowSettings::Default,
                minimum_update_interval(options),
                DirtyRegionSettings::Default,
                ColorFormat::Bgra8,
                flags,
            );
            WindowsCaptureHandler::start_free_threaded(settings).map_err(capture_start_error)
        }
        CaptureSourceKind::Window => {
            let window = window_from_source_id(&options.source_id)?;
            let settings = Settings::new(
                window,
                cursor_settings(options),
                capture_draw_border_settings(),
                SecondaryWindowSettings::Default,
                minimum_update_interval(options),
                DirtyRegionSettings::Default,
                ColorFormat::Bgra8,
                flags,
            );
            WindowsCaptureHandler::start_free_threaded(settings).map_err(capture_start_error)
        }
    }
}

fn frame_to_video_frame(frame: &mut Frame, output_size: Option<(u32, u32)>) -> Result<VideoFrame> {
    let width = frame.width();
    let height = frame.height();
    let buffer = frame.buffer().map_err(runtime_error)?;
    let mut packed = Vec::new();
    let data = buffer.as_nopadding_buffer(&mut packed);
    let (width, height, data) = if let Some((target_width, target_height)) = output_size {
        if target_width != width || target_height != height {
            (
                target_width,
                target_height,
                resize_bgra_nearest(data, width, height, target_width, target_height),
            )
        } else {
            (width, height, data.to_vec())
        }
    } else {
        (width, height, data.to_vec())
    };

    Ok(VideoFrame {
        width,
        height,
        pixel_format: PixelFormat::Bgra,
        timestamp_ns: now_ns(),
        data: Arc::from(data),
    })
}

fn frame_to_thumbnail(frame: &mut Frame) -> Result<String> {
    let width = frame.width();
    let height = frame.height();
    let buffer = frame.buffer().map_err(runtime_error)?;
    let mut packed = Vec::new();
    let data = buffer.as_nopadding_buffer(&mut packed);
    let (width, height, data) = resize_rgba_for_thumbnail(data, width, height);

    encode_png_base64(&data, width, height).ok_or_else(|| {
        Error::new(
            CaptureErrorCode::CaptureRuntimeFailed,
            "failed to encode Windows capture thumbnail",
            true,
        )
    })
}

fn capture_thumbnail<T>(item: T) -> Option<String>
where
    T: TryInto<windows_capture::settings::GraphicsCaptureItemType> + Send + 'static,
{
    let (sender, receiver) = std_mpsc::channel();
    let settings = Settings::new(
        item,
        CursorCaptureSettings::WithoutCursor,
        capture_draw_border_settings(),
        SecondaryWindowSettings::Default,
        MinimumUpdateIntervalSettings::Default,
        DirtyRegionSettings::Default,
        ColorFormat::Rgba8,
        ThumbnailCaptureFlags { sender },
    );
    let control = match ThumbnailCaptureHandler::start_free_threaded(settings) {
        Ok(control) => control,
        Err(error) => {
            eprintln!("[screen-capture] Windows thumbnail capture start failed: {error}");
            return None;
        }
    };
    let result = receiver
        .recv_timeout(THUMBNAIL_TIMEOUT)
        .map_err(|error| {
            eprintln!("[screen-capture] Windows thumbnail capture timed out or closed: {error}");
            error
        })
        .ok()
        .and_then(|result| {
            result
                .map_err(|error| {
                    eprintln!("[screen-capture] Windows thumbnail capture failed: {error:?}");
                    error
                })
                .ok()
        });
    let _ = control.stop();
    result
}

fn capture_monitor_thumbnail(monitor: Monitor) -> Option<String> {
    if let Some(thumbnail) = capture_monitor_thumbnail_gdi(monitor) {
        return Some(thumbnail);
    }

    match capture_monitor_thumbnail_result(monitor) {
        Ok(thumbnail) => Some(thumbnail),
        Err(error) => {
            eprintln!("[screen-capture] Windows DXGI display thumbnail failed: {error:?}");
            None
        }
    }
}

fn capture_monitor_thumbnail_result(monitor: Monitor) -> Result<String> {
    let mut duplication = DxgiDuplicationApi::new(monitor).map_err(runtime_error)?;

    for _ in 0..5 {
        match duplication.acquire_next_frame(60) {
            Ok(mut frame) => {
                let buffer = frame.buffer().map_err(runtime_error)?;
                let width = buffer.width();
                let height = buffer.height();
                let format = buffer.format();
                let mut packed = Vec::new();
                let data = buffer.as_nopadding_buffer(&mut packed);
                let rgba = dxgi_to_rgba(data, format).ok_or_else(|| {
                    Error::new(
                        CaptureErrorCode::CaptureRuntimeFailed,
                        format!("unsupported Windows DXGI thumbnail format: {format:?}"),
                        true,
                    )
                })?;
                let (width, height, rgba) = resize_rgba_for_thumbnail(&rgba, width, height);

                return encode_png_base64(&rgba, width, height).ok_or_else(|| {
                    Error::new(
                        CaptureErrorCode::CaptureRuntimeFailed,
                        "failed to encode Windows DXGI display thumbnail",
                        true,
                    )
                });
            }
            Err(windows_capture::dxgi_duplication_api::Error::Timeout) => continue,
            Err(error) => return Err(runtime_error(error)),
        }
    }

    Err(Error::new(
        CaptureErrorCode::CaptureRuntimeFailed,
        "Windows DXGI display thumbnail timed out",
        true,
    ))
}

fn capture_monitor_thumbnail_gdi(monitor: Monitor) -> Option<String> {
    capture_monitor_thumbnail_gdi_result(monitor)
        .map_err(|error| {
            eprintln!("[screen-capture] Windows GDI display thumbnail failed: {error:?}");
            error
        })
        .ok()
}

fn capture_monitor_thumbnail_gdi_result(monitor: Monitor) -> Result<String> {
    let rect = monitor_rect(monitor)?;
    let source_width =
        u32::try_from(rect.right.saturating_sub(rect.left)).map_err(runtime_error)?;
    let source_height =
        u32::try_from(rect.bottom.saturating_sub(rect.top)).map_err(runtime_error)?;
    if source_width == 0 || source_height == 0 {
        return Err(runtime_error("monitor has empty bounds"));
    }

    let (thumb_width, thumb_height) = thumbnail_size(source_width, source_height);
    let mut bits = std::ptr::null_mut();
    let bitmap_info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: i32::try_from(thumb_width).map_err(runtime_error)?,
            biHeight: -i32::try_from(thumb_height).map_err(runtime_error)?,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };

    let screen_dc = unsafe { GetDC(None) };
    if screen_dc.is_invalid() {
        return Err(runtime_error(
            "GetDC returned an invalid screen device context",
        ));
    }

    let memory_dc = unsafe { CreateCompatibleDC(Some(screen_dc)) };
    if memory_dc.is_invalid() {
        unsafe {
            ReleaseDC(None, screen_dc);
        }
        return Err(runtime_error(
            "CreateCompatibleDC returned an invalid memory context",
        ));
    }

    let bitmap = match unsafe {
        CreateDIBSection(
            Some(screen_dc),
            &bitmap_info,
            DIB_RGB_COLORS,
            &mut bits,
            None,
            0,
        )
    } {
        Ok(bitmap) => bitmap,
        Err(error) => {
            unsafe {
                let _ = DeleteDC(memory_dc);
                ReleaseDC(None, screen_dc);
            }
            return Err(runtime_error(error));
        }
    };

    let old_object = unsafe { SelectObject(memory_dc, HGDIOBJ(bitmap.0)) };
    let bitblt_result = unsafe {
        BitBlt(
            memory_dc,
            0,
            0,
            i32::try_from(thumb_width).map_err(runtime_error)?,
            i32::try_from(thumb_height).map_err(runtime_error)?,
            Some(screen_dc),
            rect.left,
            rect.top,
            SRCCOPY,
        )
    };

    let result = if let Err(error) = bitblt_result {
        Err(runtime_error(error))
    } else if bits.is_null() {
        Err(runtime_error(
            "CreateDIBSection returned a null pixel pointer",
        ))
    } else {
        let len = thumb_width as usize * thumb_height as usize * 4;
        let bgra = unsafe { std::slice::from_raw_parts(bits.cast::<u8>(), len) };
        let rgba = bgra_to_rgba(bgra);
        encode_png_base64(&rgba, thumb_width, thumb_height).ok_or_else(|| {
            Error::new(
                CaptureErrorCode::CaptureRuntimeFailed,
                "failed to encode Windows GDI display thumbnail",
                true,
            )
        })
    };

    unsafe {
        SelectObject(memory_dc, old_object);
        let _ = DeleteObject(HGDIOBJ(bitmap.0));
        let _ = DeleteDC(memory_dc);
        ReleaseDC(None, screen_dc);
    }

    result
}

fn enumerate_displays(include_thumbnails: bool) -> Result<Vec<CaptureSource>> {
    let monitors = Monitor::enumerate().map_err(source_error)?;
    Ok(monitors
        .into_iter()
        .enumerate()
        .filter_map(|(index, monitor)| display_source(index, monitor, include_thumbnails))
        .collect())
}

fn display_source(
    index: usize,
    monitor: Monitor,
    include_thumbnail: bool,
) -> Option<CaptureSource> {
    let width = monitor.width().ok()?;
    let height = monitor.height().ok()?;
    if width == 0 || height == 0 {
        return None;
    }

    let device_name = monitor
        .device_name()
        .unwrap_or_else(|_| format!("DISPLAY{}", index + 1));
    let name = monitor
        .name()
        .ok()
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| format!("Display {}", index + 1));

    let is_primary = monitor.index().ok() == Some(1);
    let thumbnail_base64 = include_thumbnail
        .then(|| capture_monitor_thumbnail(monitor))
        .flatten();

    Some(CaptureSource {
        id: format!("display:{device_name}:{}", index + 1),
        kind: CaptureSourceKind::Display,
        name: name.clone(),
        title: Some(name),
        app_name: None,
        pid: None,
        width,
        height,
        scale_factor: 1.0,
        is_primary,
        thumbnail_base64,
        filtered_reason: None,
    })
}

fn enumerate_windows(include_thumbnails: bool) -> Result<Vec<CaptureSource>> {
    let windows = Window::enumerate().map_err(source_error)?;
    Ok(windows
        .into_iter()
        .enumerate()
        .filter_map(|(index, window)| {
            window_source(window, include_thumbnails && index < WINDOW_THUMBNAIL_LIMIT)
        })
        .collect())
}

fn window_source(window: Window, include_thumbnail: bool) -> Option<CaptureSource> {
    let title = window.title().ok()?;
    if title.trim().is_empty() {
        return None;
    }

    let width = u32::try_from(window.width().ok()?.max(0)).ok()?;
    let height = u32::try_from(window.height().ok()?.max(0)).ok()?;
    if width == 0 || height == 0 {
        return None;
    }

    let pid = window.process_id().ok();
    let app_name = window
        .process_name()
        .ok()
        .map(|name| name.trim_end_matches(".exe").to_string())
        .filter(|name| !name.is_empty());

    let thumbnail_base64 = include_thumbnail
        .then(|| capture_thumbnail(window))
        .flatten();

    Some(CaptureSource {
        id: format!("window:{:x}", window.as_raw_hwnd() as usize),
        kind: CaptureSourceKind::Window,
        name: title.clone(),
        title: Some(title),
        app_name,
        pid,
        width,
        height,
        scale_factor: 1.0,
        is_primary: false,
        thumbnail_base64,
        filtered_reason: None,
    })
}

fn monitor_from_source_id(source_id: &str) -> Result<Monitor> {
    let index = parse_display_index(source_id)
        .ok_or_else(|| source_error(format!("invalid Windows display source id: {source_id}")))?;
    Monitor::from_index(index + 1).map_err(source_error)
}

fn window_from_source_id(source_id: &str) -> Result<Window> {
    let hwnd = parse_window_id(source_id)
        .ok_or_else(|| source_error(format!("invalid Windows window source id: {source_id}")))?;
    Ok(Window::from_raw_hwnd(hwnd as *mut std::ffi::c_void))
}

fn parse_display_index(source_id: &str) -> Option<usize> {
    source_id
        .rsplit_once(':')
        .and_then(|(_, index)| index.parse::<usize>().ok())
        .and_then(|index| index.checked_sub(1))
}

fn parse_window_id(source_id: &str) -> Option<usize> {
    source_id
        .strip_prefix("window:")
        .and_then(|id| usize::from_str_radix(id, 16).ok())
}

fn cursor_settings(options: &StartCaptureOptions) -> CursorCaptureSettings {
    if options.effective_capture_cursor() {
        CursorCaptureSettings::WithCursor
    } else {
        CursorCaptureSettings::WithoutCursor
    }
}

fn capture_draw_border_settings() -> DrawBorderSettings {
    DrawBorderSettings::WithoutBorder
}

fn minimum_update_interval(options: &StartCaptureOptions) -> MinimumUpdateIntervalSettings {
    let fps = options.effective_fps();
    MinimumUpdateIntervalSettings::Custom(Duration::from_secs_f64(1.0 / f64::from(fps)))
}

fn requested_output_size(options: &StartCaptureOptions) -> Option<(u32, u32)> {
    let width = options.width?;
    let height = options.height?;
    Some((even_dimension(width), even_dimension(height)))
}

fn even_dimension(value: u32) -> u32 {
    value.max(2) & !1
}

fn resize_bgra_nearest(
    input: &[u8],
    source_width: u32,
    source_height: u32,
    target_width: u32,
    target_height: u32,
) -> Vec<u8> {
    let mut output = vec![0u8; target_width as usize * target_height as usize * 4];
    for y in 0..target_height {
        let source_y = y.saturating_mul(source_height) / target_height;
        for x in 0..target_width {
            let source_x = x.saturating_mul(source_width) / target_width;
            let source_offset = (source_y as usize * source_width as usize + source_x as usize) * 4;
            let target_offset = (y as usize * target_width as usize + x as usize) * 4;
            output[target_offset..target_offset + 4]
                .copy_from_slice(&input[source_offset..source_offset + 4]);
        }
    }
    output
}

fn dxgi_to_rgba(data: &[u8], format: DxgiDuplicationFormat) -> Option<Vec<u8>> {
    match format {
        DxgiDuplicationFormat::Rgba8 | DxgiDuplicationFormat::Rgba8Srgb => Some(data.to_vec()),
        DxgiDuplicationFormat::Bgra8 | DxgiDuplicationFormat::Bgra8Srgb => Some(bgra_to_rgba(data)),
        _ => None,
    }
}

fn bgra_to_rgba(data: &[u8]) -> Vec<u8> {
    let mut rgba = Vec::with_capacity(data.len());
    for pixel in data.chunks_exact(4) {
        rgba.extend_from_slice(&[pixel[2], pixel[1], pixel[0], pixel[3]]);
    }
    rgba
}

fn resize_rgba_for_thumbnail(data: &[u8], width: u32, height: u32) -> (u32, u32, Vec<u8>) {
    let (target_width, target_height) = thumbnail_size(width, height);
    if target_width == width && target_height == height {
        return (width, height, data.to_vec());
    }

    (
        target_width,
        target_height,
        resize_rgba_nearest(data, width, height, target_width, target_height),
    )
}

fn thumbnail_size(width: u32, height: u32) -> (u32, u32) {
    if width <= THUMBNAIL_MAX_WIDTH {
        return (width, height);
    }
    let target_width = THUMBNAIL_MAX_WIDTH;
    let target_height = height
        .saturating_mul(target_width)
        .checked_div(width.max(1))
        .unwrap_or(height)
        .max(1);
    (target_width, target_height)
}

fn resize_rgba_nearest(
    input: &[u8],
    source_width: u32,
    source_height: u32,
    target_width: u32,
    target_height: u32,
) -> Vec<u8> {
    let mut output = vec![0u8; target_width as usize * target_height as usize * 4];
    for y in 0..target_height {
        let source_y = y.saturating_mul(source_height) / target_height;
        for x in 0..target_width {
            let source_x = x.saturating_mul(source_width) / target_width;
            let source_offset = (source_y as usize * source_width as usize + source_x as usize) * 4;
            let target_offset = (y as usize * target_width as usize + x as usize) * 4;
            output[target_offset..target_offset + 4]
                .copy_from_slice(&input[source_offset..source_offset + 4]);
        }
    }
    output
}

fn monitor_rect(monitor: Monitor) -> Result<windows::Win32::Foundation::RECT> {
    let mut info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    let ok = unsafe { GetMonitorInfoW(HMONITOR(monitor.as_raw_hmonitor()), &mut info).as_bool() };
    if ok {
        Ok(info.rcMonitor)
    } else {
        Err(runtime_error("GetMonitorInfoW failed for monitor"))
    }
}

fn now_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos().min(u128::from(u64::MAX)) as u64)
        .unwrap_or_default()
}

fn capture_start_error(error: GraphicsCaptureApiError<String>) -> Error {
    Error::new(
        CaptureErrorCode::CaptureStartFailed,
        format!("Windows capture start failed: {error}"),
        true,
    )
}

fn capture_control_error(error: impl std::fmt::Display) -> Error {
    Error::new(
        CaptureErrorCode::CaptureRuntimeFailed,
        format!("Windows capture stop failed: {error}"),
        true,
    )
}

fn runtime_error(error: impl std::fmt::Display) -> Error {
    Error::new(
        CaptureErrorCode::CaptureRuntimeFailed,
        format!("Windows capture frame failed: {error}"),
        true,
    )
}

fn source_error(error: impl std::fmt::Display) -> Error {
    Error::new(CaptureErrorCode::SourceUnavailable, error.to_string(), true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_sessions_do_not_request_windows_system_border() {
        assert_eq!(
            capture_draw_border_settings(),
            DrawBorderSettings::WithoutBorder
        );
    }
}
