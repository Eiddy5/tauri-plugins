use std::{
    collections::HashMap,
    sync::mpsc as std_mpsc,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, OnceLock,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use async_trait::async_trait;
use fast_image_resize::{
    images::{Image, ImageRef},
    FilterType, PixelType, ResizeAlg, ResizeOptions, Resizer,
};
use tokio::{
    sync::{mpsc, watch},
    task::JoinHandle,
};
use windows::Win32::{
    Foundation::HWND,
    Graphics::Gdi::{
        BitBlt, CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC,
        GetMonitorInfoW, GetWindowDC, ReleaseDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER,
        BI_RGB, DIB_RGB_COLORS, HGDIOBJ, HMONITOR, MONITORINFO, SRCCOPY,
    },
    Storage::Xps::{PrintWindow, PRINT_WINDOW_FLAGS},
    UI::WindowsAndMessaging::PW_RENDERFULLCONTENT,
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
const WINDOW_THUMBNAIL_CACHE_TTL: Duration = Duration::from_secs(5);
const WINDOW_THUMBNAIL_WGC_FALLBACK_LIMIT: usize = 3;

static WINDOW_THUMBNAIL_CACHE: OnceLock<Mutex<WindowThumbnailCache>> = OnceLock::new();

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
    let paused = Arc::new(AtomicBool::new(false));
    let (frame_sender, mut frame_receiver) = mpsc::channel(1);
    let (finish_sender, _) = watch::channel(None);
    let control = start_windows_capture(
        &options,
        Arc::clone(&paused),
        frame_sender,
        finish_sender.clone(),
    )?;

    let task = tokio::spawn(async move {
        while let Some(mut frame) = frame_receiver.recv().await {
            frame.timestamp_ns = now_ns();
            if let Err(error) = consumer.push_frame(frame).await {
                eprintln!("[screen-capture] Windows frame publish failed: {error:?}");
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

        let control = self
            .control
            .lock()
            .ok()
            .and_then(|mut control| control.take());
        if let Some(control) = control {
            control.stop().map_err(capture_control_error)?;
        }
        self.task.abort();

        Ok(())
    }
}

impl Drop for WindowsRunningCapture {
    fn drop(&mut self) {
        self.paused.store(true, Ordering::SeqCst);

        if let Ok(mut control) = self.control.lock() {
            if let Some(control) = control.take() {
                let _ = control.stop();
            }
        }
        self.task.abort();
    }
}

struct WindowsCaptureHandler {
    frame_sender: mpsc::Sender<VideoFrame>,
    finish_sender: watch::Sender<Option<crate::models::CaptureErrorPayload>>,
    paused: Arc<AtomicBool>,
    output_size: Option<(u32, u32)>,
}

struct WindowsCaptureFlags {
    frame_sender: mpsc::Sender<VideoFrame>,
    finish_sender: watch::Sender<Option<crate::models::CaptureErrorPayload>>,
    paused: Arc<AtomicBool>,
    output_size: Option<(u32, u32)>,
}

impl GraphicsCaptureApiHandler for WindowsCaptureHandler {
    type Flags = WindowsCaptureFlags;
    type Error = String;

    fn new(ctx: Context<Self::Flags>) -> std::result::Result<Self, Self::Error> {
        Ok(Self {
            frame_sender: ctx.flags.frame_sender,
            finish_sender: ctx.flags.finish_sender,
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

        if self.frame_sender.capacity() == 0 {
            return Ok(());
        }

        match frame_to_video_frame(frame, self.output_size) {
            Ok(video_frame) => match self.frame_sender.try_send(video_frame) {
                Ok(()) | Err(mpsc::error::TrySendError::Full(_)) => {}
                Err(mpsc::error::TrySendError::Closed(_)) => {
                    return Err("Windows capture frame consumer stopped".to_string());
                }
            },
            Err(error) => {
                let _ = self.finish_sender.send(Some(error.payload()));
                return Err(error.to_string());
            }
        }
        Ok(())
    }

    fn on_closed(&mut self) -> std::result::Result<(), Self::Error> {
        let error = Error::new(
            CaptureErrorCode::SourceUnavailable,
            "Windows capture source was closed",
            true,
        );
        let _ = self.finish_sender.send(Some(error.payload()));
        Ok(())
    }
}

struct ThumbnailCaptureHandler {
    sender: std_mpsc::Sender<Result<String>>,
}

struct ThumbnailCaptureFlags {
    sender: std_mpsc::Sender<Result<String>>,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct WindowThumbnailCacheKey {
    hwnd: usize,
    width: u32,
    height: u32,
    title: String,
}

struct WindowThumbnailCacheEntry {
    thumbnail: String,
    captured_at: Instant,
}

#[derive(Default)]
struct WindowThumbnailCache {
    entries: HashMap<WindowThumbnailCacheKey, WindowThumbnailCacheEntry>,
}

impl WindowThumbnailCache {
    fn get(&self, key: &WindowThumbnailCacheKey, now: Instant) -> Option<String> {
        let entry = self.entries.get(key)?;
        let age = now
            .checked_duration_since(entry.captured_at)
            .unwrap_or_default();
        (age <= WINDOW_THUMBNAIL_CACHE_TTL).then(|| entry.thumbnail.clone())
    }

    fn get_stale(&self, key: &WindowThumbnailCacheKey) -> Option<String> {
        self.entries.get(key).map(|entry| entry.thumbnail.clone())
    }

    fn insert(&mut self, key: WindowThumbnailCacheKey, thumbnail: String, captured_at: Instant) {
        self.entries.insert(
            key,
            WindowThumbnailCacheEntry {
                thumbnail,
                captured_at,
            },
        );
    }
}

struct WindowThumbnailWgcFallbackBudget {
    remaining: usize,
}

impl Default for WindowThumbnailWgcFallbackBudget {
    fn default() -> Self {
        Self {
            remaining: WINDOW_THUMBNAIL_WGC_FALLBACK_LIMIT,
        }
    }
}

impl WindowThumbnailWgcFallbackBudget {
    fn take(&mut self) -> bool {
        if self.remaining == 0 {
            return false;
        }

        self.remaining -= 1;
        true
    }
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
    frame_sender: mpsc::Sender<VideoFrame>,
    finish_sender: watch::Sender<Option<crate::models::CaptureErrorPayload>>,
) -> Result<WindowsCaptureControl> {
    let flags = WindowsCaptureFlags {
        frame_sender,
        finish_sender,
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
            let resized = resize_bgra_hamming(data, width, height, target_width, target_height)?;
            (target_width, target_height, resized)
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

fn capture_window_thumbnail(
    window: Window,
    key: &WindowThumbnailCacheKey,
    wgc_fallback_budget: &mut WindowThumbnailWgcFallbackBudget,
) -> Option<String> {
    let now = Instant::now();
    if let Some(thumbnail) = cached_window_thumbnail(key, now) {
        return Some(thumbnail);
    }

    if let Some(thumbnail) = capture_window_thumbnail_gdi(window) {
        store_window_thumbnail(key.clone(), thumbnail.clone(), now);
        return Some(thumbnail);
    }

    if wgc_fallback_budget.take() {
        if let Some(thumbnail) = capture_thumbnail(window) {
            store_window_thumbnail(key.clone(), thumbnail.clone(), now);
            return Some(thumbnail);
        }
    }

    stale_window_thumbnail(key)
}

fn cached_window_thumbnail(key: &WindowThumbnailCacheKey, now: Instant) -> Option<String> {
    WINDOW_THUMBNAIL_CACHE
        .get_or_init(|| Mutex::new(WindowThumbnailCache::default()))
        .lock()
        .ok()
        .and_then(|cache| cache.get(key, now))
}

fn stale_window_thumbnail(key: &WindowThumbnailCacheKey) -> Option<String> {
    WINDOW_THUMBNAIL_CACHE
        .get_or_init(|| Mutex::new(WindowThumbnailCache::default()))
        .lock()
        .ok()
        .and_then(|cache| cache.get_stale(key))
}

fn store_window_thumbnail(key: WindowThumbnailCacheKey, thumbnail: String, captured_at: Instant) {
    if let Ok(mut cache) = WINDOW_THUMBNAIL_CACHE
        .get_or_init(|| Mutex::new(WindowThumbnailCache::default()))
        .lock()
    {
        cache.insert(key, thumbnail, captured_at);
    }
}

fn capture_window_thumbnail_gdi(window: Window) -> Option<String> {
    capture_window_thumbnail_gdi_result(window)
        .map_err(|error| {
            eprintln!("[screen-capture] Windows GDI window thumbnail failed: {error:?}");
            error
        })
        .ok()
}

fn capture_window_thumbnail_gdi_result(window: Window) -> Result<String> {
    let source_width =
        u32::try_from(window.width().map_err(runtime_error)?.max(0)).map_err(runtime_error)?;
    let source_height =
        u32::try_from(window.height().map_err(runtime_error)?.max(0)).map_err(runtime_error)?;
    if source_width == 0 || source_height == 0 {
        return Err(runtime_error("window has empty bounds"));
    }

    let hwnd = HWND(window.as_raw_hwnd());
    if hwnd.is_invalid() {
        return Err(runtime_error("window has an invalid HWND"));
    }

    let mut bits = std::ptr::null_mut();
    let bitmap_info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: i32::try_from(source_width).map_err(runtime_error)?,
            biHeight: -i32::try_from(source_height).map_err(runtime_error)?,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };

    let window_dc = unsafe { GetWindowDC(Some(hwnd)) };
    if window_dc.is_invalid() {
        return Err(runtime_error(
            "GetWindowDC returned an invalid window device context",
        ));
    }

    let memory_dc = unsafe { CreateCompatibleDC(Some(window_dc)) };
    if memory_dc.is_invalid() {
        unsafe {
            ReleaseDC(Some(hwnd), window_dc);
        }
        return Err(runtime_error(
            "CreateCompatibleDC returned an invalid memory context",
        ));
    }

    let bitmap = match unsafe {
        CreateDIBSection(
            Some(window_dc),
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
                ReleaseDC(Some(hwnd), window_dc);
            }
            return Err(runtime_error(error));
        }
    };

    let old_object = unsafe { SelectObject(memory_dc, HGDIOBJ(bitmap.0)) };
    let mut captured = false;
    for flags in [
        PRINT_WINDOW_FLAGS(PW_RENDERFULLCONTENT),
        PRINT_WINDOW_FLAGS(0),
        PRINT_WINDOW_FLAGS(4),
    ] {
        if unsafe { PrintWindow(hwnd, memory_dc, flags).as_bool() } {
            captured = true;
            break;
        }
    }

    if !captured {
        captured = unsafe {
            BitBlt(
                memory_dc,
                0,
                0,
                i32::try_from(source_width).map_err(runtime_error)?,
                i32::try_from(source_height).map_err(runtime_error)?,
                Some(window_dc),
                0,
                0,
                SRCCOPY,
            )
            .is_ok()
        };
    }

    let result = if !captured {
        Err(runtime_error("PrintWindow and BitBlt failed for window"))
    } else if bits.is_null() {
        Err(runtime_error(
            "CreateDIBSection returned a null pixel pointer",
        ))
    } else {
        let len = source_width as usize * source_height as usize * 4;
        let bgra = unsafe { std::slice::from_raw_parts(bits.cast::<u8>(), len) };
        let rgba = bgra_to_opaque_rgba(bgra);
        let (width, height, rgba) = resize_rgba_for_thumbnail(&rgba, source_width, source_height);
        encode_png_base64(&rgba, width, height).ok_or_else(|| {
            Error::new(
                CaptureErrorCode::CaptureRuntimeFailed,
                "failed to encode Windows GDI window thumbnail",
                true,
            )
        })
    };

    unsafe {
        SelectObject(memory_dc, old_object);
        let _ = DeleteObject(HGDIOBJ(bitmap.0));
        let _ = DeleteDC(memory_dc);
        ReleaseDC(Some(hwnd), window_dc);
    }

    result
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
    let mut wgc_fallback_budget = WindowThumbnailWgcFallbackBudget::default();
    Ok(windows
        .into_iter()
        .filter_map(|window| window_source(window, include_thumbnails, &mut wgc_fallback_budget))
        .collect())
}

fn window_source(
    window: Window,
    include_thumbnail: bool,
    wgc_fallback_budget: &mut WindowThumbnailWgcFallbackBudget,
) -> Option<CaptureSource> {
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

    let hwnd = window.as_raw_hwnd() as usize;
    let thumbnail_key = WindowThumbnailCacheKey {
        hwnd,
        width,
        height,
        title: title.clone(),
    };
    let thumbnail_base64 = include_thumbnail
        .then(|| capture_window_thumbnail(window, &thumbnail_key, wgc_fallback_budget))
        .flatten();

    Some(CaptureSource {
        id: format!("window:{hwnd:x}"),
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

fn resize_bgra_hamming(
    input: &[u8],
    source_width: u32,
    source_height: u32,
    target_width: u32,
    target_height: u32,
) -> Result<Vec<u8>> {
    let source = ImageRef::new(source_width, source_height, input, PixelType::U8x4)
        .map_err(runtime_error)?;
    let mut destination = Image::new(target_width, target_height, PixelType::U8x4);
    let mut resizer = Resizer::new();
    let options = ResizeOptions::new()
        .resize_alg(ResizeAlg::Convolution(FilterType::Hamming))
        .use_alpha(false);
    resizer
        .resize(&source, &mut destination, &options)
        .map_err(runtime_error)?;
    Ok(destination.into_vec())
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

fn bgra_to_opaque_rgba(data: &[u8]) -> Vec<u8> {
    let mut rgba = Vec::with_capacity(data.len());
    for pixel in data.chunks_exact(4) {
        rgba.extend_from_slice(&[pixel[2], pixel[1], pixel[0], 255]);
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

    #[test]
    fn hamming_downscale_antialiases_high_frequency_screen_edges() {
        let mut input = Vec::new();
        for _ in 0..2 {
            for value in [0, 255, 0, 255] {
                input.extend_from_slice(&[value, value, value, 255]);
            }
        }

        let output = resize_bgra_hamming(&input, 4, 2, 2, 2).expect("resize BGRA test pattern");

        assert_eq!(output.len(), 2 * 2 * 4);
        assert!(output.chunks_exact(4).all(|pixel| {
            (1..=254).contains(&pixel[0])
                && pixel[0] == pixel[1]
                && pixel[1] == pixel[2]
                && pixel[3] == 255
        }));
    }

    #[test]
    fn window_thumbnail_cache_reuses_only_fresh_entries() {
        let mut cache = WindowThumbnailCache::default();
        let key = WindowThumbnailCacheKey {
            hwnd: 42,
            width: 1280,
            height: 720,
            title: "Document".to_string(),
        };
        let captured_at = std::time::Instant::now();

        cache.insert(key.clone(), "png-a".to_string(), captured_at);

        assert_eq!(
            cache.get(&key, captured_at + Duration::from_secs(2)),
            Some("png-a".to_string())
        );
        assert_eq!(cache.get(&key, captured_at + Duration::from_secs(10)), None);
    }

    #[test]
    fn window_thumbnail_wgc_fallback_budget_is_limited() {
        let mut budget = WindowThumbnailWgcFallbackBudget::default();

        let allowed = (0..5).map(|_| budget.take()).collect::<Vec<bool>>();

        assert_eq!(allowed, vec![true, true, true, false, false]);
    }
}
