use std::{
    cell::RefCell,
    collections::HashMap,
    sync::mpsc,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex, OnceLock,
    },
    time::Duration,
};

use async_trait::async_trait;
use objc2::{define_class, msg_send, rc::Retained, AnyThread, DefinedClass, MainThreadOnly};
use objc2_app_kit::{
    NSBackingStoreType, NSBezierPath, NSColor, NSCompositingOperation, NSEvent, NSGraphicsContext,
    NSImage, NSImageInterpolation, NSPanel, NSStatusWindowLevel, NSView, NSWindingRule,
    NSWindowAnimationBehavior, NSWindowCollectionBehavior, NSWindowStyleMask,
};
use objc2_core_foundation::{
    CFArray, CFDictionary, CFMutableData, CFNumber, CFRetained, CFString, CFType, Type,
};
use objc2_core_graphics::{
    kCGNullWindowID, kCGWindowAlpha, kCGWindowBounds, kCGWindowLayer, CGDisplayBounds, CGError,
    CGEvent, CGGetActiveDisplayList, CGImage, CGMainDisplayID,
    CGRectMakeWithDictionaryRepresentation, CGWindowListCopyWindowInfo, CGWindowListOption,
};
use objc2_foundation::{
    MainThreadMarker, NSError, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString,
};
use objc2_image_io::CGImageDestination;
use objc2_screen_capture_kit::SCScreenshotManager;
use tauri::{AppHandle, Runtime};
use tokio::sync::oneshot;

use crate::{
    error::{Error, ErrorCode},
    models::{CaptureOptions, CaptureRegion},
    selection::{Interaction, Point, Rect, SelectionModel, Size},
    Result,
};

use super::{
    desktop::{DesktopRect, WindowCatalog},
    magnifier::{layout_magnifier, MagnifierLayout},
    NativeCaptureAdapter, NativeCaptureImage, NativeCaptureOutcome,
};

const ESCAPE_KEY_CODE: u16 = 53;
const RETURN_KEY_CODE: u16 = 36;
const KEYPAD_ENTER_KEY_CODE: u16 = 76;
const HANDLE_SIZE: f64 = 8.0;

static NEXT_SESSION_ID: AtomicU64 = AtomicU64::new(1);

pub(crate) struct MacOsNativeCaptureAdapter<R: Runtime> {
    app: AppHandle<R>,
}

impl<R: Runtime> MacOsNativeCaptureAdapter<R> {
    pub(crate) fn new(app: AppHandle<R>) -> Self {
        Self { app }
    }
}

#[async_trait]
impl<R: Runtime> NativeCaptureAdapter for MacOsNativeCaptureAdapter<R> {
    async fn capture_area(&self, options: CaptureOptions) -> Result<NativeCaptureOutcome> {
        if !objc2::available!(macos = 15.2) {
            return Err(Error::new(
                ErrorCode::Unsupported,
                "当前原生截图实现需要 macOS 15.2 或更高版本",
                false,
            ));
        }
        ensure_screen_capture_permission()?;
        let desktop = virtual_desktop()?;
        let desktop_bounds = desktop.bounds;
        let frame_task = tokio::task::spawn_blocking(move || capture_desktop(desktop_bounds));
        let windows_task = tokio::task::spawn_blocking(move || mac_window_catalog(desktop_bounds));
        let frame = frame_task.await.map_err(|error| {
            Error::new(
                ErrorCode::CaptureFailed,
                format!("macOS 截图任务异常结束：{error}"),
                true,
            )
        })??;
        let windows = windows_task.await.unwrap_or_default();
        // 浮层直接复用 CGImage，避免在用户开始框选前进行整屏 PNG 编解码。
        let preview_image = frame.image.clone();
        let session_id = NEXT_SESSION_ID.fetch_add(1, Ordering::Relaxed);
        let (completion, outcome) = oneshot::channel();
        let mut model = SelectionModel::new_with_scale(
            frame.logical_size(),
            Size::new(
                options.effective_min_width(),
                options.effective_min_height(),
            ),
            1.0,
        );
        let cursor = cursor_desktop_point(desktop_bounds);
        if let Some(point) = cursor {
            let _ = model.hover_window(windows.window_at(point));
        }
        let session = Arc::new(MacSelectionSession {
            model: Mutex::new(model),
            windows,
            cursor: Mutex::new(cursor),
            completion: Mutex::new(Some(completion)),
        });
        register_session(session_id, Arc::clone(&session));
        let mut guard = MacSessionGuard::new(self.app.clone(), session_id);
        let display_frames = desktop.displays;
        let logical_size = frame.logical_size();

        run_on_main_thread(&self.app, move || {
            show_overlay(
                session_id,
                preview_image,
                desktop_bounds,
                display_frames,
                logical_size,
            )
        })
        .await?;

        let selection = outcome.await.map_err(|_| {
            Error::new(
                ErrorCode::OverlayFailed,
                "macOS 截图浮层在返回结果前关闭",
                true,
            )
        })?;
        guard.close().await?;

        match selection {
            MacSelectionOutcome::Cancelled => Ok(NativeCaptureOutcome::Cancelled),
            MacSelectionOutcome::Selected(selection) => {
                let image = tokio::task::spawn_blocking(move || frame.crop_png(selection))
                    .await
                    .map_err(|error| {
                        Error::new(
                            ErrorCode::CaptureFailed,
                            format!("macOS 截图裁剪任务异常结束：{error}"),
                            true,
                        )
                    })??;
                Ok(NativeCaptureOutcome::Captured(image))
            }
        }
    }
}

fn ensure_screen_capture_permission() -> Result<()> {
    if unsafe { CGPreflightScreenCaptureAccess() } {
        return Ok(());
    }
    if unsafe { CGRequestScreenCaptureAccess() } {
        Ok(())
    } else {
        Err(Error::new(
            ErrorCode::PermissionDenied,
            "未获得 macOS 屏幕录制权限，请在系统设置中授权后重试",
            true,
        ))
    }
}

#[derive(Clone, Debug)]
struct MacDesktop {
    bounds: DesktopRect,
    displays: Vec<DesktopRect>,
}

fn virtual_desktop() -> Result<MacDesktop> {
    let mut display_count = 0;
    let status = unsafe { CGGetActiveDisplayList(0, std::ptr::null_mut(), &mut display_count) };
    if status != CGError::Success || display_count == 0 {
        return Err(Error::new(
            ErrorCode::DisplayUnavailable,
            "无法读取 macOS 活跃显示器列表",
            true,
        ));
    }

    let mut displays = vec![0; display_count as usize];
    let status =
        unsafe { CGGetActiveDisplayList(display_count, displays.as_mut_ptr(), &mut display_count) };
    if status != CGError::Success {
        return Err(Error::new(
            ErrorCode::DisplayUnavailable,
            "读取 macOS 活跃显示器边界失败",
            true,
        ));
    }
    displays.truncate(display_count as usize);
    let displays = displays
        .into_iter()
        .map(|display_id| {
            let frame = CGDisplayBounds(display_id);
            DesktopRect::new(
                frame.origin.x,
                frame.origin.y,
                frame.size.width,
                frame.size.height,
            )
        })
        .filter(|display| display.is_valid())
        .collect::<Vec<_>>();
    let bounds = DesktopRect::union_all(displays.iter().copied()).ok_or_else(|| {
        Error::new(
            ErrorCode::DisplayUnavailable,
            "macOS 虚拟桌面边界为空",
            true,
        )
    })?;
    Ok(MacDesktop { bounds, displays })
}

fn cursor_desktop_point(desktop: DesktopRect) -> Option<Point> {
    let event = CGEvent::new(None)?;
    let point = CGEvent::location(Some(&event));
    desktop.local_point(point.x, point.y)
}

#[derive(Clone)]
struct MacCapturedFrame {
    image: CFRetained<CGImage>,
    pixel_width: u32,
    pixel_height: u32,
    logical_width: f64,
    logical_height: f64,
}

impl MacCapturedFrame {
    fn new(image: CFRetained<CGImage>, logical_width: f64, logical_height: f64) -> Result<Self> {
        let pixel_width = u32::try_from(CGImage::width(Some(&image))).map_err(|_| {
            Error::new(
                ErrorCode::CaptureFailed,
                "macOS 截图宽度超出可处理范围",
                false,
            )
        })?;
        let pixel_height = u32::try_from(CGImage::height(Some(&image))).map_err(|_| {
            Error::new(
                ErrorCode::CaptureFailed,
                "macOS 截图高度超出可处理范围",
                false,
            )
        })?;
        if pixel_width == 0
            || pixel_height == 0
            || !logical_width.is_finite()
            || !logical_height.is_finite()
            || logical_width <= 0.0
            || logical_height <= 0.0
        {
            return Err(Error::new(
                ErrorCode::CaptureFailed,
                "macOS 截图帧尺寸无效",
                true,
            ));
        }

        Ok(Self {
            image,
            pixel_width,
            pixel_height,
            logical_width,
            logical_height,
        })
    }

    fn logical_size(&self) -> Size {
        Size::new(self.logical_width, self.logical_height)
    }

    fn crop_png(&self, logical_region: Rect) -> Result<NativeCaptureImage> {
        let region = self.physical_region(logical_region)?;
        let crop_rect = NSRect::new(
            NSPoint::new(f64::from(region.x), f64::from(region.y)),
            NSSize::new(f64::from(region.width), f64::from(region.height)),
        );
        let image = CGImage::with_image_in_rect(Some(&self.image), crop_rect).ok_or_else(|| {
            Error::new(
                ErrorCode::CaptureFailed,
                "macOS 无法裁剪选中的截图区域",
                true,
            )
        })?;
        let png = encode_cg_image_png(&image)?;

        Ok(NativeCaptureImage {
            png,
            width: region.width,
            height: region.height,
            region,
        })
    }

    fn physical_region(&self, logical_region: Rect) -> Result<CaptureRegion> {
        if !logical_region.x.is_finite()
            || !logical_region.y.is_finite()
            || !logical_region.width.is_finite()
            || !logical_region.height.is_finite()
            || logical_region.width <= 0.0
            || logical_region.height <= 0.0
        {
            return Err(invalid_selection("选区无效"));
        }

        let scale_x = f64::from(self.pixel_width) / self.logical_width;
        let scale_y = f64::from(self.pixel_height) / self.logical_height;
        let left = (logical_region.x * scale_x)
            .floor()
            .clamp(0.0, f64::from(self.pixel_width)) as u32;
        let top = (logical_region.y * scale_y)
            .floor()
            .clamp(0.0, f64::from(self.pixel_height)) as u32;
        let right = (logical_region.right() * scale_x)
            .ceil()
            .clamp(0.0, f64::from(self.pixel_width)) as u32;
        let bottom = (logical_region.bottom() * scale_y)
            .ceil()
            .clamp(0.0, f64::from(self.pixel_height)) as u32;
        let width = right.saturating_sub(left);
        let height = bottom.saturating_sub(top);
        if width == 0 || height == 0 {
            return Err(invalid_selection("选区映射到物理像素后为空"));
        }

        Ok(CaptureRegion {
            x: left,
            y: top,
            width,
            height,
        })
    }
}

fn capture_desktop(desktop: DesktopRect) -> Result<MacCapturedFrame> {
    if !desktop.is_valid() {
        return Err(Error::new(
            ErrorCode::DisplayUnavailable,
            "macOS 虚拟桌面边界无效",
            true,
        ));
    }
    let capture_frame = NSRect::new(
        NSPoint::new(desktop.x, desktop.y),
        NSSize::new(desktop.width, desktop.height),
    );

    let (sender, receiver) = mpsc::sync_channel(1);
    let completion = block2::RcBlock::new(move |image: *mut CGImage, error: *mut NSError| {
        let result = retain_cg_image(image, error);
        let _ = sender.send(result);
    });
    unsafe {
        SCScreenshotManager::captureImageInRect_completionHandler(capture_frame, Some(&completion));
    }
    let image = receiver
        .recv_timeout(Duration::from_secs(15))
        .map_err(|error| {
            Error::new(
                ErrorCode::CaptureFailed,
                format!("等待 ScreenCaptureKit 截图超时：{error}"),
                true,
            )
        })?
        .map_err(|message| Error::new(ErrorCode::CaptureFailed, message, true))?;
    MacCapturedFrame::new(image, desktop.width, desktop.height)
}

fn retain_cg_image(
    image: *mut CGImage,
    error: *mut NSError,
) -> std::result::Result<CFRetained<CGImage>, String> {
    if !error.is_null() {
        let description = unsafe { &*error }.localizedDescription().to_string();
        return Err(format!("ScreenCaptureKit 截图失败：{description}"));
    }
    if image.is_null() {
        return Err("ScreenCaptureKit 返回了空图像".to_owned());
    }

    Ok(unsafe { &*image }.retain())
}

fn encode_cg_image_png(image: &CGImage) -> Result<Vec<u8>> {
    let data = CFMutableData::new(None, 0).ok_or_else(|| {
        Error::new(
            ErrorCode::CaptureFailed,
            "macOS 无法分配 PNG 输出缓冲区",
            true,
        )
    })?;
    let png_type = CFString::from_static_str("public.png");
    let destination = unsafe { CGImageDestination::with_data(&data, &png_type, 1, None) }
        .ok_or_else(|| Error::new(ErrorCode::CaptureFailed, "macOS 无法创建 PNG 编码器", true))?;
    unsafe {
        destination.add_image(image, None);
    }
    if !unsafe { destination.finalize() } {
        return Err(Error::new(
            ErrorCode::CaptureFailed,
            "macOS PNG 编码失败",
            true,
        ));
    }
    Ok(data.to_vec())
}

fn invalid_selection(message: impl Into<String>) -> Error {
    Error::new(ErrorCode::InvalidSelection, message, true)
}

fn mac_window_catalog(desktop: DesktopRect) -> WindowCatalog {
    let options =
        CGWindowListOption::OptionOnScreenOnly | CGWindowListOption::ExcludeDesktopElements;
    let Some(window_info) = CGWindowListCopyWindowInfo(options, kCGNullWindowID) else {
        return WindowCatalog::default();
    };
    let window_info: &CFArray<CFDictionary<CFString, CFType>> =
        unsafe { window_info.cast_unchecked() };
    let windows = window_info.iter().filter_map(|entry| {
        let layer = dictionary_number(&entry, unsafe { kCGWindowLayer })?;
        if layer != 0.0 {
            return None;
        }
        if dictionary_number(&entry, unsafe { kCGWindowAlpha }).is_some_and(|alpha| alpha <= 0.01) {
            return None;
        }
        let bounds = entry
            .get(unsafe { kCGWindowBounds })?
            .downcast::<CFDictionary>()
            .ok()?;
        let mut frame = NSRect::default();
        if !unsafe { CGRectMakeWithDictionaryRepresentation(Some(&bounds), &mut frame) } {
            return None;
        }
        Some(DesktopRect::new(
            frame.origin.x,
            frame.origin.y,
            frame.size.width,
            frame.size.height,
        ))
    });
    WindowCatalog::from_absolute(desktop, windows)
}

fn dictionary_number(dictionary: &CFDictionary<CFString, CFType>, key: &CFString) -> Option<f64> {
    dictionary.get(key)?.downcast::<CFNumber>().ok()?.as_f64()
}

fn appkit_display_frame(display: DesktopRect) -> NSRect {
    let main = CGDisplayBounds(CGMainDisplayID());
    NSRect::new(
        NSPoint::new(display.x, main.size.height - display.y - display.height),
        NSSize::new(display.width, display.height),
    )
}

enum MacSelectionOutcome {
    Selected(Rect),
    Cancelled,
}

struct MacSelectionSession {
    model: Mutex<SelectionModel>,
    windows: WindowCatalog,
    cursor: Mutex<Option<Point>>,
    completion: Mutex<Option<oneshot::Sender<MacSelectionOutcome>>>,
}

impl MacSelectionSession {
    fn set_cursor(&self, point: Point) -> Option<Point> {
        self.cursor
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .replace(point)
    }

    fn handle_interaction(&self, session_id: u64, interaction: Interaction) {
        match interaction {
            Interaction::Confirm(selection) => {
                hide_overlay(session_id);
                self.finish(MacSelectionOutcome::Selected(selection));
            }
            Interaction::Cancel => {
                hide_overlay(session_id);
                self.finish(MacSelectionOutcome::Cancelled);
            }
            Interaction::Changed => redraw_overlay(session_id),
            Interaction::Unchanged => {}
        }
    }

    fn finish(&self, outcome: MacSelectionOutcome) {
        if let Some(completion) = self
            .completion
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
        {
            let _ = completion.send(outcome);
        }
    }
}

fn sessions() -> &'static Mutex<HashMap<u64, Arc<MacSelectionSession>>> {
    static SESSIONS: OnceLock<Mutex<HashMap<u64, Arc<MacSelectionSession>>>> = OnceLock::new();
    SESSIONS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn register_session(id: u64, session: Arc<MacSelectionSession>) {
    sessions()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(id, session);
}

fn unregister_session(id: u64) {
    sessions()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .remove(&id);
}

fn session(id: u64) -> Option<Arc<MacSelectionSession>> {
    sessions()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(&id)
        .cloned()
}

#[derive(Clone, Copy)]
struct ScreenshotViewIvars {
    session_id: u64,
    origin_x: f64,
    origin_y: f64,
    desktop_height: f64,
}

define_class!(
  #[unsafe(super(NSView))]
  #[thread_kind = MainThreadOnly]
  #[ivars = ScreenshotViewIvars]
  struct ScreenshotView;

  unsafe impl NSObjectProtocol for ScreenshotView {}

  impl ScreenshotView {
    #[unsafe(method(mouseDown:))]
    fn mouse_down(&self, event: &NSEvent) {
      if let Some(window) = self.window() {
        window.makeKeyWindow();
        let _ = window.makeFirstResponder(Some(self));
      }
      self.with_model(event, |model, point| model.pointer_down(point));
    }

    #[unsafe(method(mouseDragged:))]
    fn mouse_dragged(&self, event: &NSEvent) {
      self.with_model(event, |model, point| model.pointer_move(point));
    }

    #[unsafe(method(mouseMoved:))]
    fn mouse_moved(&self, event: &NSEvent) {
      self.hover_window(event);
    }

    #[unsafe(method(mouseUp:))]
    fn mouse_up(&self, event: &NSEvent) {
      self.with_model(event, |model, point| model.pointer_up(point));
    }

    #[unsafe(method(rightMouseDown:))]
    fn right_mouse_down(&self, _event: &NSEvent) {
      if let Some(session) = session(self.ivars().session_id) {
        session.handle_interaction(self.ivars().session_id, Interaction::Cancel);
      }
    }

    #[unsafe(method(keyDown:))]
    fn key_down(&self, event: &NSEvent) {
      let Some(session) = session(self.ivars().session_id) else {
        return;
      };
      let interaction = match event.keyCode() {
        ESCAPE_KEY_CODE => {
          let model = session
            .model
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
          model.cancel()
        }
        RETURN_KEY_CODE | KEYPAD_ENTER_KEY_CODE => {
          let model = session
            .model
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
          model.confirm()
        }
        _ => Interaction::Unchanged,
      };
      session.handle_interaction(self.ivars().session_id, interaction);
    }

    #[unsafe(method(acceptsFirstMouse:))]
    fn accepts_first_mouse(&self, _event: Option<&NSEvent>) -> bool {
      true
    }

    #[unsafe(method(acceptsFirstResponder))]
    fn accepts_first_responder(&self) -> bool {
      true
    }

    #[unsafe(method(drawRect:))]
    fn draw_rect(&self, _dirty_rect: NSRect) {
      draw_overlay(self);
    }
  }
);

define_class!(
  #[unsafe(super(NSPanel))]
  #[thread_kind = MainThreadOnly]
  struct ScreenshotPanel;

  unsafe impl NSObjectProtocol for ScreenshotPanel {}

  impl ScreenshotPanel {
    #[unsafe(method(canBecomeKeyWindow))]
    fn can_become_key_window(&self) -> bool {
      true
    }
  }
);

impl ScreenshotPanel {
    fn new(frame: NSRect, mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(());
        let panel: Retained<Self> = unsafe {
            msg_send![
              super(this),
              initWithContentRect: frame,
              styleMask: NSWindowStyleMask::Borderless,
              backing: NSBackingStoreType::Buffered,
              defer: false,
            ]
        };
        panel
    }
}

impl ScreenshotView {
    fn new(
        session_id: u64,
        frame: NSRect,
        origin: Point,
        desktop_height: f64,
        mtm: MainThreadMarker,
    ) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(ScreenshotViewIvars {
            session_id,
            origin_x: origin.x,
            origin_y: origin.y,
            desktop_height,
        });
        unsafe { msg_send![super(this), initWithFrame: frame] }
    }

    fn with_model(
        &self,
        event: &NSEvent,
        update: impl FnOnce(&mut SelectionModel, Point) -> Interaction,
    ) {
        let Some(session) = session(self.ivars().session_id) else {
            return;
        };
        let point = self.event_point(event);
        let previous_cursor = session.set_cursor(point);
        let (interaction, magnifier_visible) = {
            let mut model = session
                .model
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let interaction = update(&mut model, point);
            (interaction, model.magnifier_visible())
        };
        if interaction == Interaction::Unchanged && magnifier_visible {
            redraw_magnifier(self.ivars().session_id, previous_cursor, Some(point));
            return;
        }
        session.handle_interaction(self.ivars().session_id, interaction);
    }

    fn hover_window(&self, event: &NSEvent) {
        let Some(session) = session(self.ivars().session_id) else {
            return;
        };
        let point = self.event_point(event);
        let previous_cursor = session.set_cursor(point);
        let window = session.windows.window_at(point);
        let (interaction, magnifier_visible) = {
            let mut model = session
                .model
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let interaction = model.hover_window(window);
            (interaction, model.magnifier_visible())
        };
        if interaction == Interaction::Unchanged && magnifier_visible {
            redraw_magnifier(self.ivars().session_id, previous_cursor, Some(point));
            return;
        }
        session.handle_interaction(self.ivars().session_id, interaction);
    }

    fn event_point(&self, event: &NSEvent) -> Point {
        let bounds = self.bounds();
        let location = self.convertPoint_fromView(event.locationInWindow(), None);
        Point::new(
            self.ivars().origin_x + location.x,
            self.ivars().origin_y + bounds.size.height - location.y,
        )
    }
}

struct MacOverlayPanel {
    panel: Retained<ScreenshotPanel>,
    view: Retained<ScreenshotView>,
}

struct MacOverlay {
    panels: Vec<MacOverlayPanel>,
    image: Retained<NSImage>,
}

impl Drop for MacOverlay {
    fn drop(&mut self) {
        for overlay in &self.panels {
            overlay.panel.orderOut(None);
            overlay.panel.close();
        }
    }
}

thread_local! {
  static OVERLAYS: RefCell<HashMap<u64, MacOverlay>> = RefCell::new(HashMap::new());
}

fn show_overlay(
    session_id: u64,
    preview_image: CFRetained<CGImage>,
    desktop: DesktopRect,
    displays: Vec<DesktopRect>,
    logical_size: Size,
) -> Result<()> {
    let mtm = MainThreadMarker::new().ok_or_else(|| {
        Error::new(
            ErrorCode::OverlayFailed,
            "macOS 原生截图浮层只能在 AppKit 主线程创建",
            false,
        )
    })?;
    let image = NSImage::initWithCGImage_size(
        NSImage::alloc(),
        &preview_image,
        NSSize::new(logical_size.width, logical_size.height),
    );
    let cursor = cursor_desktop_point(desktop);
    if let (Some(session), Some(cursor)) = (session(session_id), cursor) {
        let _ = session.set_cursor(cursor);
        let mut model = session
            .model
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _ = model.hover_window(session.windows.window_at(cursor));
    }
    let mut key_panel_index = 0;
    let mut panels = Vec::with_capacity(displays.len());
    for (index, display) in displays.into_iter().enumerate() {
        let origin = Point::new(display.x - desktop.x, display.y - desktop.y);
        if cursor.is_some_and(|cursor| {
            cursor.x >= origin.x
                && cursor.x <= origin.x + display.width
                && cursor.y >= origin.y
                && cursor.y <= origin.y + display.height
        }) {
            key_panel_index = index;
        }
        let local_frame = NSRect::new(
            NSPoint::new(0.0, 0.0),
            NSSize::new(display.width, display.height),
        );
        let view = ScreenshotView::new(session_id, local_frame, origin, logical_size.height, mtm);
        let panel = ScreenshotPanel::new(appkit_display_frame(display), mtm);
        panel.setContentView(Some(&view));
        panel.setOpaque(true);
        panel.setHasShadow(false);
        panel.setIgnoresMouseEvents(false);
        panel.setMovable(false);
        panel.setMovableByWindowBackground(false);
        panel.setHidesOnDeactivate(false);
        panel.setBecomesKeyOnlyIfNeeded(false);
        panel.setExcludedFromWindowsMenu(true);
        panel.setAcceptsMouseMovedEvents(true);
        panel.setAnimationBehavior(NSWindowAnimationBehavior::None);
        panel.setCollectionBehavior(
            NSWindowCollectionBehavior::CanJoinAllSpaces
                | NSWindowCollectionBehavior::FullScreenAuxiliary
                | NSWindowCollectionBehavior::Stationary,
        );
        panel.setBackgroundColor(Some(&NSColor::blackColor()));
        panel.setLevel(NSStatusWindowLevel);
        panel.setTitle(&NSString::from_str("Tauri Screenshot Overlay"));
        unsafe { panel.setReleasedWhenClosed(false) };
        panels.push(MacOverlayPanel { panel, view });
    }
    if panels.is_empty() {
        return Err(Error::new(
            ErrorCode::DisplayUnavailable,
            "macOS 没有可创建截图浮层的显示器",
            true,
        ));
    }

    OVERLAYS.with(|overlays| {
        overlays
            .borrow_mut()
            .insert(session_id, MacOverlay { panels, image });
    });
    OVERLAYS.with(|overlays| {
        if let Some(overlay) = overlays.borrow().get(&session_id) {
            for panel in &overlay.panels {
                panel.panel.orderFrontRegardless();
                panel.view.setNeedsDisplay(true);
            }
            let key_panel = &overlay.panels[key_panel_index.min(overlay.panels.len() - 1)];
            key_panel.panel.makeKeyAndOrderFront(None);
            let _ = key_panel.panel.makeFirstResponder(Some(&key_panel.view));
        }
    });
    Ok(())
}

fn close_overlay(session_id: u64) {
    OVERLAYS.with(|overlays| {
        overlays.borrow_mut().remove(&session_id);
    });
}

fn hide_overlay(session_id: u64) {
    OVERLAYS.with(|overlays| {
        if let Some(overlay) = overlays.borrow().get(&session_id) {
            for panel in &overlay.panels {
                panel.panel.orderOut(None);
            }
        }
    });
}

fn redraw_overlay(session_id: u64) {
    OVERLAYS.with(|overlays| {
        if let Some(overlay) = overlays.borrow().get(&session_id) {
            for panel in &overlay.panels {
                panel.view.setNeedsDisplay(true);
            }
        }
    });
}

fn redraw_magnifier(session_id: u64, previous: Option<Point>, current: Option<Point>) {
    if previous == current {
        return;
    }
    OVERLAYS.with(|overlays| {
        let overlays = overlays.borrow();
        let Some(overlay) = overlays.get(&session_id) else {
            return;
        };
        let source = Size::new(overlay.image.size().width, overlay.image.size().height);
        for panel in &overlay.panels {
            let bounds = panel.view.bounds();
            let origin = Point::new(panel.view.ivars().origin_x, panel.view.ivars().origin_y);
            let viewport = Rect {
                x: origin.x,
                y: origin.y,
                width: bounds.size.width,
                height: bounds.size.height,
            };
            for cursor in [previous, current].into_iter().flatten() {
                let Some(layout) = layout_magnifier(cursor, cursor, viewport, source, 1.0) else {
                    continue;
                };
                let local = translate_to_view(layout.frame, origin);
                let native = top_down_rect(local, bounds.size.height);
                let padding = 4.0;
                panel.view.setNeedsDisplayInRect(NSRect::new(
                    NSPoint::new(native.origin.x - padding, native.origin.y - padding),
                    NSSize::new(
                        native.size.width + padding * 2.0,
                        native.size.height + padding * 2.0,
                    ),
                ));
            }
        }
    });
}

fn draw_overlay(view: &ScreenshotView) {
    let Some(session) = session(view.ivars().session_id) else {
        return;
    };
    let image = OVERLAYS.with(|overlays| {
        overlays
            .borrow()
            .get(&view.ivars().session_id)
            .map(|overlay| overlay.image.clone())
    });
    let Some(image) = image else {
        return;
    };
    let bounds = view.bounds();
    let origin = Point::new(view.ivars().origin_x, view.ivars().origin_y);
    let source = NSRect::new(
        NSPoint::new(
            origin.x,
            view.ivars().desktop_height - origin.y - bounds.size.height,
        ),
        bounds.size,
    );
    image.drawInRect_fromRect_operation_fraction(bounds, source, NSCompositingOperation::Copy, 1.0);

    let cursor = *session
        .cursor
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let (selection, has_committed_selection, toolbar, magnifier_focus) = {
        let model = session
            .model
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        (
            model.preview_selection(),
            model.selection().is_some(),
            model.toolbar(),
            cursor.and_then(|cursor| model.magnifier_focus(cursor)),
        )
    };
    let dim = NSColor::colorWithSRGBRed_green_blue_alpha(0.0, 0.0, 0.0, 0.42);
    dim.setFill();
    if let Some(selection) = selection {
        let local_selection = translate_to_view(selection, origin);
        if let Some(visible_selection) =
            clip_to_view(local_selection, bounds.size.width, bounds.size.height)
        {
            draw_dimmed_area(visible_selection, bounds.size.width, bounds.size.height);
            draw_selection(local_selection, bounds.size.height, has_committed_selection);
        } else {
            NSBezierPath::fillRect(bounds);
        }
    } else {
        NSBezierPath::fillRect(bounds);
    }
    if let Some(toolbar) = toolbar {
        let toolbar = translate_to_view(toolbar.frame, origin);
        if clip_to_view(toolbar, bounds.size.width, bounds.size.height).is_some() {
            draw_toolbar(toolbar, bounds.size.height);
        }
    }
    if let (Some(cursor), Some(focus)) = (cursor, magnifier_focus) {
        let viewport = Rect {
            x: origin.x,
            y: origin.y,
            width: bounds.size.width,
            height: bounds.size.height,
        };
        if let Some(layout) = layout_magnifier(
            cursor,
            focus,
            viewport,
            Size::new(image.size().width, image.size().height),
            1.0,
        ) {
            draw_magnifier(
                &image,
                layout,
                origin,
                bounds.size.height,
                view.ivars().desktop_height,
            );
        }
    }
}

fn draw_dimmed_area(selection: Rect, width: f64, height: f64) {
    let path = NSBezierPath::bezierPath();
    path.setWindingRule(NSWindingRule::EvenOdd);
    path.appendBezierPathWithRect(NSRect::new(
        NSPoint::new(0.0, 0.0),
        NSSize::new(width, height),
    ));
    path.appendBezierPathWithRect(top_down_rect(selection, height));
    path.fill();
}

fn translate_to_view(rect: Rect, origin: Point) -> Rect {
    Rect {
        x: rect.x - origin.x,
        y: rect.y - origin.y,
        ..rect
    }
}

fn clip_to_view(rect: Rect, width: f64, height: f64) -> Option<Rect> {
    let left = rect.x.max(0.0);
    let top = rect.y.max(0.0);
    let right = rect.right().min(width);
    let bottom = rect.bottom().min(height);
    (right > left && bottom > top).then_some(Rect {
        x: left,
        y: top,
        width: right - left,
        height: bottom - top,
    })
}

fn draw_selection(selection: Rect, view_height: f64, show_handles: bool) {
    let green =
        NSColor::colorWithSRGBRed_green_blue_alpha(49.0 / 255.0, 214.0 / 255.0, 132.0 / 255.0, 1.0);
    green.setStroke();
    green.setFill();
    let native = top_down_rect(selection, view_height);
    let path = NSBezierPath::bezierPathWithRect(native);
    path.setLineWidth(2.0);
    path.stroke();

    if show_handles {
        for point in [
            Point::new(selection.x, selection.y),
            Point::new(selection.right(), selection.y),
            Point::new(selection.x, selection.bottom()),
            Point::new(selection.right(), selection.bottom()),
        ] {
            NSBezierPath::fillRect(NSRect::new(
                NSPoint::new(
                    point.x - HANDLE_SIZE / 2.0,
                    view_height - point.y - HANDLE_SIZE / 2.0,
                ),
                NSSize::new(HANDLE_SIZE, HANDLE_SIZE),
            ));
        }
    }
}

fn draw_toolbar(toolbar: Rect, view_height: f64) {
    let native = top_down_rect(toolbar, view_height);
    let background =
        NSColor::colorWithSRGBRed_green_blue_alpha(25.0 / 255.0, 25.0 / 255.0, 25.0 / 255.0, 0.96);
    background.setFill();
    let path = NSBezierPath::bezierPath();
    path.appendBezierPathWithRoundedRect_xRadius_yRadius(native, 8.0, 8.0);
    path.fill();

    let white = NSColor::whiteColor();
    white.setStroke();
    let button_width = toolbar.width / 3.0;
    let native_mid_y = native.origin.y + native.size.height / 2.0;
    for index in 1..=2 {
        let x = native.origin.x + button_width * f64::from(index);
        let separator = NSBezierPath::bezierPath();
        separator.moveToPoint(NSPoint::new(x, native.origin.y + 8.0));
        separator.lineToPoint(NSPoint::new(x, native.origin.y + native.size.height - 8.0));
        separator.setLineWidth(1.0);
        separator.stroke();
    }

    let cancel_x = native.origin.x + button_width / 2.0;
    let cancel = NSBezierPath::bezierPath();
    cancel.moveToPoint(NSPoint::new(cancel_x - 5.0, native_mid_y - 5.0));
    cancel.lineToPoint(NSPoint::new(cancel_x + 5.0, native_mid_y + 5.0));
    cancel.moveToPoint(NSPoint::new(cancel_x + 5.0, native_mid_y - 5.0));
    cancel.lineToPoint(NSPoint::new(cancel_x - 5.0, native_mid_y + 5.0));
    cancel.setLineWidth(2.0);
    cancel.stroke();

    let reset_x = native.origin.x + button_width * 1.5;
    let reset = NSBezierPath::bezierPathWithRect(NSRect::new(
        NSPoint::new(reset_x - 6.0, native_mid_y - 6.0),
        NSSize::new(12.0, 12.0),
    ));
    reset.setLineWidth(2.0);
    reset.stroke();

    let confirm_x = native.origin.x + button_width * 2.5;
    let confirm = NSBezierPath::bezierPath();
    confirm.moveToPoint(NSPoint::new(confirm_x - 6.0, native_mid_y));
    confirm.lineToPoint(NSPoint::new(confirm_x - 1.0, native_mid_y - 5.0));
    confirm.lineToPoint(NSPoint::new(confirm_x + 7.0, native_mid_y + 6.0));
    confirm.setLineWidth(2.5);
    confirm.stroke();
}

fn draw_magnifier(
    image: &NSImage,
    layout: MagnifierLayout,
    origin: Point,
    view_height: f64,
    desktop_height: f64,
) {
    let frame = translate_to_view(layout.frame, origin);
    let native_frame = top_down_rect(frame, view_height);
    let background = NSBezierPath::bezierPath();
    background.appendBezierPathWithRoundedRect_xRadius_yRadius(native_frame, 8.0, 8.0);
    NSColor::whiteColor().setFill();
    background.fill();

    let inset = 3.0;
    let content = Rect {
        x: frame.x + inset,
        y: frame.y + inset,
        width: (frame.width - inset * 2.0).max(0.0),
        height: (frame.height - inset * 2.0).max(0.0),
    };
    let source = NSRect::new(
        NSPoint::new(
            layout.sample.x,
            desktop_height - layout.sample.y - layout.sample.height,
        ),
        NSSize::new(layout.sample.width, layout.sample.height),
    );
    if content.width > 0.0 && content.height > 0.0 {
        let native_content = top_down_rect(content, view_height);
        if let Some(context) = NSGraphicsContext::currentContext() {
            let interpolation = context.imageInterpolation();
            context.setImageInterpolation(NSImageInterpolation::None);
            image.drawInRect_fromRect_operation_fraction(
                native_content,
                source,
                NSCompositingOperation::Copy,
                1.0,
            );
            context.setImageInterpolation(interpolation);
        } else {
            image.drawInRect_fromRect_operation_fraction(
                native_content,
                source,
                NSCompositingOperation::Copy,
                1.0,
            );
        }

        let focus_x =
            content.x + (layout.focus.x - layout.frame.x) / layout.frame.width * content.width;
        let focus_y =
            content.y + (layout.focus.y - layout.frame.y) / layout.frame.height * content.height;
        let native_focus_y = view_height - focus_y;
        let crosshair = NSBezierPath::bezierPath();
        crosshair.moveToPoint(NSPoint::new(focus_x, native_content.origin.y));
        crosshair.lineToPoint(NSPoint::new(
            focus_x,
            native_content.origin.y + native_content.size.height,
        ));
        crosshair.moveToPoint(NSPoint::new(native_content.origin.x, native_focus_y));
        crosshair.lineToPoint(NSPoint::new(
            native_content.origin.x + native_content.size.width,
            native_focus_y,
        ));
        NSColor::colorWithSRGBRed_green_blue_alpha(7.0 / 255.0, 193.0 / 255.0, 96.0 / 255.0, 1.0)
            .setStroke();
        crosshair.setLineWidth(1.5);
        crosshair.stroke();
    }

    NSColor::colorWithSRGBRed_green_blue_alpha(218.0 / 255.0, 218.0 / 255.0, 218.0 / 255.0, 1.0)
        .setStroke();
    background.setLineWidth(1.0);
    background.stroke();
}

fn top_down_rect(rect: Rect, view_height: f64) -> NSRect {
    NSRect::new(
        NSPoint::new(rect.x, view_height - rect.y - rect.height),
        NSSize::new(rect.width.max(0.0), rect.height.max(0.0)),
    )
}

async fn run_on_main_thread<R, T>(
    app: &AppHandle<R>,
    task: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<T>
where
    R: Runtime,
    T: Send + 'static,
{
    let (sender, receiver) = oneshot::channel();
    app.run_on_main_thread(move || {
        let _ = sender.send(task());
    })
    .map_err(|error| {
        Error::new(
            ErrorCode::OverlayFailed,
            format!("无法调度 macOS AppKit 主线程任务：{error}"),
            true,
        )
    })?;
    receiver.await.map_err(|_| {
        Error::new(
            ErrorCode::OverlayFailed,
            "macOS AppKit 主线程任务未返回结果",
            true,
        )
    })?
}

struct MacSessionGuard<R: Runtime> {
    app: AppHandle<R>,
    session_id: u64,
    active: bool,
}

impl<R: Runtime> MacSessionGuard<R> {
    fn new(app: AppHandle<R>, session_id: u64) -> Self {
        Self {
            app,
            session_id,
            active: true,
        }
    }

    async fn close(&mut self) -> Result<()> {
        let session_id = self.session_id;
        run_on_main_thread(&self.app, move || {
            close_overlay(session_id);
            Ok(())
        })
        .await?;
        unregister_session(self.session_id);
        self.active = false;
        Ok(())
    }
}

impl<R: Runtime> Drop for MacSessionGuard<R> {
    fn drop(&mut self) {
        if !self.active {
            return;
        }
        unregister_session(self.session_id);
        let session_id = self.session_id;
        let _ = self.app.run_on_main_thread(move || {
            close_overlay(session_id);
        });
    }
}

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGPreflightScreenCaptureAccess() -> bool;
    fn CGRequestScreenCaptureAccess() -> bool;
}

#[cfg(test)]
mod tests {
    use super::*;
    use objc2_core_graphics::{
        CGBitmapContextCreate, CGBitmapContextCreateImage, CGColorSpace, CGImageAlphaInfo,
    };

    #[test]
    fn secondary_panel_translates_virtual_desktop_selection_to_local_coordinates() {
        let local = translate_to_view(
            Rect {
                x: 3000.0,
                y: 200.0,
                width: 600.0,
                height: 500.0,
            },
            Point::new(2560.0, 0.0),
        );

        assert_eq!(
            local,
            Rect {
                x: 440.0,
                y: 200.0,
                width: 600.0,
                height: 500.0,
            }
        );
        assert_eq!(clip_to_view(local, 2560.0, 1440.0), Some(local));
    }

    #[test]
    fn native_crop_encodes_only_the_selected_region() {
        let mut pixels = vec![
            255_u8, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255,
        ];
        let color_space = CGColorSpace::new_device_rgb().expect("device RGB color space");
        let context = unsafe {
            CGBitmapContextCreate(
                pixels.as_mut_ptr().cast(),
                2,
                2,
                8,
                8,
                Some(&color_space),
                CGImageAlphaInfo::PremultipliedLast.0,
            )
        }
        .expect("bitmap context");
        let image = CGBitmapContextCreateImage(Some(&context)).expect("CGImage");
        let frame = MacCapturedFrame::new(image, 2.0, 2.0).expect("native frame");

        let result = frame
            .crop_png(Rect {
                x: 0.0,
                y: 0.0,
                width: 1.0,
                height: 1.0,
            })
            .expect("cropped PNG");
        let decoded = image::load_from_memory(&result.png)
            .expect("decode PNG")
            .into_rgba8();

        assert_eq!((result.width, result.height), (1, 1));
        assert_eq!(decoded.get_pixel(0, 0).0, [255, 0, 0, 255]);
    }

    #[test]
    fn dimmed_area_has_no_horizontal_seam_at_fractional_selection_edges() {
        let width = 64_usize;
        let height = 48_usize;
        let mut pixels = vec![255_u8; width * height * 4];
        let color_space = CGColorSpace::new_device_rgb().expect("device RGB color space");
        let context = unsafe {
            CGBitmapContextCreate(
                pixels.as_mut_ptr().cast(),
                width,
                height,
                8,
                width * 4,
                Some(&color_space),
                CGImageAlphaInfo::PremultipliedLast.0,
            )
        }
        .expect("bitmap context");
        let graphics = NSGraphicsContext::graphicsContextWithCGContext_flipped(&context, false);
        let previous = NSGraphicsContext::currentContext();
        NSGraphicsContext::setCurrentContext(Some(&graphics));

        let selection = Rect {
            x: 16.25,
            y: 12.25,
            width: 32.0,
            height: 23.5,
        };
        NSColor::colorWithSRGBRed_green_blue_alpha(0.0, 0.0, 0.0, 0.42).setFill();
        draw_dimmed_area(selection, width as f64, height as f64);
        NSGraphicsContext::setCurrentContext(previous.as_deref());

        let outside_column = (0..height)
            .map(|y| pixels[(y * width + 4) * 4])
            .collect::<Vec<_>>();
        let darkest = outside_column.iter().copied().min().expect("darkest pixel");
        let lightest = outside_column
            .iter()
            .copied()
            .max()
            .expect("lightest pixel");

        assert!(
            lightest.saturating_sub(darkest) <= 1,
            "选区外遮罩出现横向接缝：darkest={darkest}, lightest={lightest}, rows={outside_column:?}"
        );
    }

    #[test]
    fn magnifier_draws_a_full_crosshair_at_the_layout_focus() {
        let width = 160_usize;
        let height = 160_usize;
        let color_space = CGColorSpace::new_device_rgb().expect("device RGB color space");

        let mut source_pixels = vec![255_u8; width * height * 4];
        let source_context = unsafe {
            CGBitmapContextCreate(
                source_pixels.as_mut_ptr().cast(),
                width,
                height,
                8,
                width * 4,
                Some(&color_space),
                CGImageAlphaInfo::PremultipliedLast.0,
            )
        }
        .expect("source bitmap context");
        let source_image = CGBitmapContextCreateImage(Some(&source_context)).expect("source image");
        let source_image = NSImage::initWithCGImage_size(
            NSImage::alloc(),
            &source_image,
            NSSize::new(width as f64, height as f64),
        );

        let mut output_pixels = vec![255_u8; width * height * 4];
        let output_context = unsafe {
            CGBitmapContextCreate(
                output_pixels.as_mut_ptr().cast(),
                width,
                height,
                8,
                width * 4,
                Some(&color_space),
                CGImageAlphaInfo::PremultipliedLast.0,
            )
        }
        .expect("output bitmap context");
        let graphics =
            NSGraphicsContext::graphicsContextWithCGContext_flipped(&output_context, false);
        let previous = NSGraphicsContext::currentContext();
        NSGraphicsContext::setCurrentContext(Some(&graphics));

        let layout = layout_magnifier(
            Point::new(10.0, 10.0),
            Point::new(80.0, 80.0),
            Rect {
                x: 0.0,
                y: 0.0,
                width: width as f64,
                height: height as f64,
            },
            Size::new(width as f64, height as f64),
            1.0,
        )
        .expect("magnifier layout");
        draw_magnifier(
            &source_image,
            layout,
            Point::new(0.0, 0.0),
            height as f64,
            height as f64,
        );
        NSGraphicsContext::setCurrentContext(previous.as_deref());

        let green_pixels = output_pixels
            .chunks_exact(4)
            .enumerate()
            .filter_map(|(index, pixel)| {
                (pixel[1] > 120 && pixel[1] > pixel[0].saturating_mul(2) && pixel[1] > pixel[2])
                    .then_some((index % width, index / width))
            })
            .collect::<Vec<_>>();
        let min_x = green_pixels
            .iter()
            .map(|(x, _)| *x)
            .min()
            .expect("green crosshair");
        let max_x = green_pixels.iter().map(|(x, _)| *x).max().unwrap();
        let min_y = green_pixels.iter().map(|(_, y)| *y).min().unwrap();
        let max_y = green_pixels.iter().map(|(_, y)| *y).max().unwrap();

        assert!(max_x - min_x >= 100, "横向十字线未贯穿放大区域");
        assert!(max_y - min_y >= 100, "纵向十字线未贯穿放大区域");
    }

    #[test]
    fn selection_border_is_continuous_green() {
        let width = 64_usize;
        let height = 48_usize;
        let mut pixels = vec![255_u8; width * height * 4];
        let color_space = CGColorSpace::new_device_rgb().expect("device RGB color space");
        let context = unsafe {
            CGBitmapContextCreate(
                pixels.as_mut_ptr().cast(),
                width,
                height,
                8,
                width * 4,
                Some(&color_space),
                CGImageAlphaInfo::PremultipliedLast.0,
            )
        }
        .expect("bitmap context");
        let graphics = NSGraphicsContext::graphicsContextWithCGContext_flipped(&context, false);
        let previous = NSGraphicsContext::currentContext();
        NSGraphicsContext::setCurrentContext(Some(&graphics));

        draw_selection(
            Rect {
                x: 8.0,
                y: 8.0,
                width: 48.0,
                height: 32.0,
            },
            height as f64,
            false,
        );
        NSGraphicsContext::setCurrentContext(previous.as_deref());

        for x in 10..54 {
            let has_green = (7..=9).any(|y| {
                let pixel = &pixels[(y * width + x) * 4..][..4];
                pixel[1] > 150 && pixel[1] > pixel[0].saturating_mul(2) && pixel[1] > pixel[2]
            });
            assert!(has_green, "选框在 x={x} 处不是连续绿色实线");
        }
    }
}
