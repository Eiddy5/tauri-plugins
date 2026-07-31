use std::{
    cell::RefCell,
    collections::HashMap,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex, OnceLock,
    },
};

use objc2::{define_class, msg_send, rc::Retained, AnyThread, DefinedClass, MainThreadOnly};
#[cfg(test)]
use objc2_app_kit::NSGraphicsContext;
use objc2_app_kit::{
    NSBackingStoreType, NSBezierPath, NSColor, NSCompositingOperation, NSEvent, NSImage, NSPanel,
    NSStatusWindowLevel, NSView, NSWindowAnimationBehavior, NSWindowCollectionBehavior,
    NSWindowStyleMask,
};
use objc2_core_foundation::CFRetained;
use objc2_core_graphics::{CGDisplayBounds, CGImage, CGMainDisplayID};
use objc2_foundation::{MainThreadMarker, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString};
use tauri::{AppHandle, Runtime};
use tokio::sync::oneshot;

use crate::{
    error::{Error, ErrorCode},
    native::{
        desktop::{DesktopRect, WindowCatalog},
        magnifier::layout_magnifier,
    },
    selection::{Interaction, Point, Rect, SelectionModel, Size},
    Result,
};

use super::{
    capture::cursor_desktop_point,
    drawing::{
        clip_to_view, draw_dimmed_area, draw_magnifier, draw_selection, draw_toolbar,
        top_down_rect, translate_to_view,
    },
};

const ESCAPE_KEY_CODE: u16 = 53;
const RETURN_KEY_CODE: u16 = 36;
const KEYPAD_ENTER_KEY_CODE: u16 = 76;

static NEXT_SESSION_ID: AtomicU64 = AtomicU64::new(1);

fn appkit_display_frame(display: DesktopRect) -> NSRect {
    let main = CGDisplayBounds(CGMainDisplayID());
    NSRect::new(
        NSPoint::new(display.x, main.size.height - display.y - display.height),
        NSSize::new(display.width, display.height),
    )
}

pub(super) async fn select_region<R: Runtime>(
    app: &AppHandle<R>,
    preview_image: CFRetained<CGImage>,
    desktop: DesktopRect,
    displays: Vec<DesktopRect>,
    logical_size: Size,
    mut model: SelectionModel,
    windows: WindowCatalog,
) -> Result<Option<Rect>> {
    let session_id = NEXT_SESSION_ID.fetch_add(1, Ordering::Relaxed);
    let (completion, outcome) = oneshot::channel();
    let cursor = cursor_desktop_point(desktop);
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
    let mut guard = MacSessionGuard::new(app.clone(), session_id);

    run_on_main_thread(app, move || {
        show_overlay(session_id, preview_image, desktop, displays, logical_size)
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

    Ok(match selection {
        MacSelectionOutcome::Selected(selection) => Some(selection),
        MacSelectionOutcome::Cancelled => None,
    })
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
