use std::{
    ffi::{c_int, c_void, CString},
    ptr,
    sync::{Arc, Mutex, OnceLock, Weak},
    time::{Duration, Instant},
};

use async_trait::async_trait;
use core_foundation_sys::{
    array::{CFArrayGetCount, CFArrayGetValueAtIndex, CFArrayRef},
    base::{kCFAllocatorDefault, Boolean, CFEqual, CFIndex, CFRelease, CFRetain, CFTypeRef},
    number::{CFBooleanGetValue, CFBooleanRef},
    runloop::{
        kCFRunLoopDefaultMode, CFRunLoopAddSource, CFRunLoopGetMain, CFRunLoopRemoveSource,
        CFRunLoopSourceRef,
    },
    string::{kCFStringEncodingUTF8, CFStringCreateWithCString, CFStringRef},
};
use dispatch2::{run_on_main, DispatchQueue, DispatchTime, MainThreadBound};
use objc2::{msg_send, rc::Retained, MainThreadMarker};
use objc2_app_kit::{
    NSBackingStoreType, NSColor, NSScreen, NSStatusWindowLevel, NSWindow,
    NSWindowCollectionBehavior, NSWindowSharingType, NSWindowStyleMask,
};
use objc2_foundation::{ns_string, NSDictionary, NSPoint, NSRect, NSSize, NSUInteger};

use crate::{
    error::Error,
    models::{CaptureErrorCode, CaptureSourceKind},
    overlay::{
        corner_segments, OverlayRect, OverlaySegment, OverlayStyle, OverlayTarget, ShareOverlay,
    },
    Result,
};

type AXError = i32;
type AXUIElementRef = *const c_void;
type AXObserverRef = *const c_void;
type AXValueRef = *const c_void;
type AXValueType = u32;
type AXObserverCallback = extern "C" fn(
    observer: AXObserverRef,
    element: AXUIElementRef,
    notification: CFStringRef,
    refcon: *mut c_void,
);

const AX_SUCCESS: AXError = 0;
const AX_VALUE_CGPOINT: AXValueType = 1;
const AX_VALUE_CGSIZE: AXValueType = 2;
const OVERLAY_SETTLE_DELAY: Duration = Duration::from_millis(150);

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct CGPoint {
    x: f64,
    y: f64,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct CGSize {
    width: f64,
    height: f64,
}

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXUIElementCreateApplication(pid: c_int) -> AXUIElementRef;
    fn AXUIElementCopyAttributeValue(
        element: AXUIElementRef,
        attribute: CFStringRef,
        value: *mut CFTypeRef,
    ) -> AXError;
    fn AXObserverCreate(
        application: c_int,
        callback: AXObserverCallback,
        out_observer: *mut AXObserverRef,
    ) -> AXError;
    fn AXObserverAddNotification(
        observer: AXObserverRef,
        element: AXUIElementRef,
        notification: CFStringRef,
        refcon: *mut c_void,
    ) -> AXError;
    fn AXObserverRemoveNotification(
        observer: AXObserverRef,
        element: AXUIElementRef,
        notification: CFStringRef,
    ) -> AXError;
    fn AXObserverGetRunLoopSource(observer: AXObserverRef) -> CFRunLoopSourceRef;
    fn AXValueGetValue(
        value: AXValueRef,
        value_type: AXValueType,
        out_value: *mut c_void,
    ) -> Boolean;
    fn _AXUIElementGetWindow(element: AXUIElementRef, identifier: *mut u32) -> AXError;
}

#[derive(Default)]
pub struct MacOsShareOverlay {
    style: OverlayStyle,
    state: Mutex<Option<OverlayMode>>,
}

enum OverlayMode {
    Native {
        overlay: NativeOverlay,
        rect: OverlayRect,
    },
    Window(WindowOverlayTracker),
    PendingWindow(PendingWindowOverlay),
}

#[async_trait]
impl ShareOverlay for MacOsShareOverlay {
    async fn start(&self, target: OverlayTarget) -> Result<()> {
        self.stop_current()?;

        let mode = match target.source_kind {
            CaptureSourceKind::Display => match display_rect_from_source_id(&target.source_id) {
                Ok(rect) => {
                    let overlay = NativeOverlay::new(self.style)?;
                    overlay.show(rect)?;
                    Some(OverlayMode::Native { overlay, rect })
                }
                Err(error) => {
                    eprintln!("[screen-capture] failed to start macOS display overlay: {error:?}");
                    None
                }
            },
            CaptureSourceKind::Window => {
                let Some(target) = window_overlay_target(&target.source_id) else {
                    return Ok(());
                };
                if !accessibility_is_trusted() {
                    let _ = request_accessibility_prompt();
                    Some(OverlayMode::PendingWindow(PendingWindowOverlay::new(
                        target, self.style,
                    )))
                } else {
                    match WindowOverlayTracker::new(target, self.style) {
                        Ok(tracker) => Some(OverlayMode::Window(tracker)),
                        Err(error) => {
                            eprintln!(
                                "[screen-capture] failed to start macOS window overlay: {error:?}"
                            );
                            None
                        }
                    }
                }
            }
        };

        *self.lock_state()? = mode;
        Ok(())
    }

    async fn show(&self) -> Result<()> {
        match self.lock_state()?.as_ref() {
            Some(OverlayMode::Native { overlay, rect }) => overlay.show(*rect)?,
            Some(OverlayMode::Window(tracker)) => tracker.show_again(),
            Some(OverlayMode::PendingWindow(pending)) => pending.show_again(),
            None => {}
        }
        Ok(())
    }

    async fn hide(&self) -> Result<()> {
        match self.lock_state()?.as_ref() {
            Some(OverlayMode::Native { overlay, .. }) => overlay.hide(),
            Some(OverlayMode::Window(tracker)) => tracker.hide(),
            Some(OverlayMode::PendingWindow(pending)) => pending.hide(),
            None => {}
        }
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        self.stop_current()
    }
}

impl MacOsShareOverlay {
    fn stop_current(&self) -> Result<()> {
        if let Some(mode) = self.lock_state()?.take() {
            match mode {
                OverlayMode::Native { overlay, .. } => overlay.close(),
                OverlayMode::Window(tracker) => tracker.close(),
                OverlayMode::PendingWindow(pending) => pending.close(),
            }
        }
        Ok(())
    }

    fn lock_state(&self) -> Result<std::sync::MutexGuard<'_, Option<OverlayMode>>> {
        self.state
            .lock()
            .map_err(|_| internal_error("macOS overlay lock was poisoned"))
    }
}

#[derive(Clone)]
struct NativeOverlay {
    inner: Arc<Mutex<MainThreadBound<NativeOverlayInner>>>,
}

impl NativeOverlay {
    fn new(style: OverlayStyle) -> Result<Self> {
        let inner = run_on_main(move |mtm| NativeOverlayInner::new(style, mtm))?;
        Ok(Self {
            inner: Arc::new(Mutex::new(inner)),
        })
    }

    fn show(&self, rect: OverlayRect) -> Result<()> {
        run_on_main(|mtm| {
            self.inner
                .lock()
                .map_err(|_| internal_error("macOS overlay lock was poisoned"))?
                .get_mut(mtm)
                .show(rect);
            Ok(())
        })
    }

    fn hide(&self) {
        run_on_main(|mtm| {
            if let Ok(mut inner) = self.inner.lock() {
                inner.get_mut(mtm).hide();
            }
        });
    }

    fn close(&self) {
        run_on_main(|mtm| {
            if let Ok(mut inner) = self.inner.lock() {
                inner.get_mut(mtm).close();
            }
        });
    }
}

struct NativeOverlayInner {
    style: OverlayStyle,
    segments: Vec<Retained<NSWindow>>,
}

impl NativeOverlayInner {
    fn new(style: OverlayStyle, mtm: MainThreadMarker) -> Result<MainThreadBound<Self>> {
        let color = ns_color_from_rgb(style.color);
        let mut segments = Vec::with_capacity(8);
        for _ in 0..8 {
            segments.push(create_segment_window(&color, mtm));
        }

        Ok(MainThreadBound::new(Self { style, segments }, mtm))
    }

    fn show(&mut self, rect: OverlayRect) {
        for (window, segment) in self
            .segments
            .iter()
            .zip(corner_segments(rect, self.style).into_iter())
        {
            window.setFrame_display(ns_rect(segment), true);
            window.orderFrontRegardless();
        }
    }

    fn hide(&mut self) {
        for window in &self.segments {
            window.orderOut(None);
        }
    }

    fn close(&mut self) {
        for window in &self.segments {
            window.close();
        }
    }
}

fn create_segment_window(color: &NSColor, mtm: MainThreadMarker) -> Retained<NSWindow> {
    let window = unsafe {
        NSWindow::initWithContentRect_styleMask_backing_defer(
            mtm.alloc(),
            NSRect::ZERO,
            NSWindowStyleMask::Borderless,
            NSBackingStoreType::Buffered,
            false,
        )
    };
    window.setBackgroundColor(Some(color));
    window.setOpaque(false);
    window.setIgnoresMouseEvents(true);
    window.setLevel(NSStatusWindowLevel);
    window.setCanHide(false);
    window.setHidesOnDeactivate(false);
    window.setHasShadow(false);
    unsafe {
        window.setReleasedWhenClosed(false);
        window.setSharingType(NSWindowSharingType::None);
        window.setCollectionBehavior(
            NSWindowCollectionBehavior::CanJoinAllSpaces
                | NSWindowCollectionBehavior::FullScreenAuxiliary
                | NSWindowCollectionBehavior::Stationary
                | NSWindowCollectionBehavior::IgnoresCycle,
        );
    }
    window
}

fn ns_color_from_rgb(rgb: u32) -> Retained<NSColor> {
    let red = f64::from((rgb >> 16) & 0xff) / 255.0;
    let green = f64::from((rgb >> 8) & 0xff) / 255.0;
    let blue = f64::from(rgb & 0xff) / 255.0;
    NSColor::colorWithSRGBRed_green_blue_alpha(red, green, blue, 1.0)
}

fn ns_rect(segment: OverlaySegment) -> NSRect {
    NSRect::new(
        NSPoint::new(f64::from(segment.x), f64::from(segment.y)),
        NSSize::new(f64::from(segment.width), f64::from(segment.height)),
    )
}

fn display_rect_from_source_id(source_id: &str) -> Result<OverlayRect> {
    let display_id = parse_source_id(source_id, "display")?;
    run_on_main(move |mtm| display_rect_for_display_id(display_id, mtm))
}

fn display_rect_for_display_id(display_id: u32, mtm: MainThreadMarker) -> Result<OverlayRect> {
    let screens = NSScreen::screens(mtm);
    let key = ns_string!("NSScreenNumber");

    for index in 0..screens.count() {
        let screen = screens.objectAtIndex(index);
        let description = screen.deviceDescription();
        let Some(value) = NSDictionary::objectForKey(&description, &key) else {
            continue;
        };
        let screen_number: NSUInteger = unsafe { msg_send![&value, unsignedIntegerValue] };
        if u32::try_from(screen_number).ok() == Some(display_id) {
            return Ok(overlay_rect_from_ns_rect(screen.frame()));
        }
    }

    Err(Error::new(
        CaptureErrorCode::SourceNotFound,
        format!("macOS display was not found for overlay: {display_id}"),
        true,
    ))
}

#[derive(Debug, Clone)]
struct WindowOverlayTarget {
    window_id: u32,
    pid: i32,
    rect: OverlayRect,
}

#[cfg(feature = "macos-screencapturekit")]
fn window_overlay_target(source_id: &str) -> Option<WindowOverlayTarget> {
    let window_id = parse_source_id(source_id, "window").ok()?;
    let content = screencapturekit::prelude::SCShareableContent::create()
        .with_on_screen_windows_only(true)
        .with_exclude_desktop_windows(true)
        .get()
        .ok()?;
    let window = content
        .windows()
        .into_iter()
        .find(|window| window.window_id() == window_id)?;
    let pid = window.owning_application()?.process_id();
    let frame = window.frame();

    Some(WindowOverlayTarget {
        window_id,
        pid,
        rect: overlay_rect_from_top_left(frame.x, frame.y, frame.width, frame.height),
    })
}

#[cfg(not(feature = "macos-screencapturekit"))]
fn window_overlay_target(_source_id: &str) -> Option<WindowOverlayTarget> {
    None
}

struct PendingWindowOverlay {
    state: Arc<Mutex<PendingWindowOverlayState>>,
}

impl PendingWindowOverlay {
    fn new(target: WindowOverlayTarget, style: OverlayStyle) -> Self {
        let state = Arc::new(Mutex::new(PendingWindowOverlayState {
            target,
            style,
            tracker: None,
            active: true,
        }));
        pending_overlays()
            .lock()
            .map(|mut overlays| overlays.push(Arc::downgrade(&state)))
            .ok();
        Self { state }
    }

    fn show_again(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.retry_attach();
            if let Some(tracker) = &state.tracker {
                tracker.show_again();
            }
        }
    }

    fn hide(&self) {
        if let Ok(state) = self.state.lock() {
            if let Some(tracker) = &state.tracker {
                tracker.hide();
            }
        }
    }

    fn close(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.close();
        }
    }
}

impl Drop for PendingWindowOverlay {
    fn drop(&mut self) {
        self.close();
    }
}

struct PendingWindowOverlayState {
    target: WindowOverlayTarget,
    style: OverlayStyle,
    tracker: Option<WindowOverlayTracker>,
    active: bool,
}

impl PendingWindowOverlayState {
    fn retry_attach(&mut self) {
        if !self.active || self.tracker.is_some() || !accessibility_is_trusted() {
            return;
        }

        match WindowOverlayTracker::new(self.target.clone(), self.style) {
            Ok(tracker) => self.tracker = Some(tracker),
            Err(error) => {
                eprintln!(
                    "[screen-capture] failed to attach pending macOS window overlay: {error:?}"
                );
            }
        }
    }

    fn close(&mut self) {
        self.active = false;
        if let Some(tracker) = self.tracker.take() {
            tracker.close();
        }
    }
}

pub fn retry_pending_window_overlays() {
    let mut overlays = match pending_overlays().lock() {
        Ok(overlays) => overlays,
        Err(_) => return,
    };
    overlays.retain(|overlay| overlay.strong_count() > 0);
    for overlay in overlays.iter().filter_map(Weak::upgrade) {
        if let Ok(mut overlay) = overlay.lock() {
            overlay.retry_attach();
        }
    }
}

fn pending_overlays() -> &'static Mutex<Vec<Weak<Mutex<PendingWindowOverlayState>>>> {
    static PENDING: OnceLock<Mutex<Vec<Weak<Mutex<PendingWindowOverlayState>>>>> = OnceLock::new();
    PENDING.get_or_init(|| Mutex::new(Vec::new()))
}

struct WindowOverlayTracker {
    state: Arc<Mutex<WindowOverlayState>>,
}

impl WindowOverlayTracker {
    fn new(target: WindowOverlayTarget, style: OverlayStyle) -> Result<Self> {
        run_on_main(move |mtm| WindowOverlayState::start(target, style, mtm))
            .map(|state| Self { state })
    }

    fn show_again(&self) {
        run_on_main({
            let state = Arc::clone(&self.state);
            move |_| {
                if let Ok(mut state) = state.lock() {
                    state.redraw_if_available();
                }
            }
        });
    }

    fn hide(&self) {
        run_on_main({
            let state = Arc::clone(&self.state);
            move |_| {
                if let Ok(mut state) = state.lock() {
                    state.hide();
                }
            }
        });
    }

    fn close(&self) {
        run_on_main({
            let state = Arc::clone(&self.state);
            move |_| {
                if let Ok(mut state) = state.lock() {
                    state.close();
                }
            }
        });
    }
}

impl Drop for WindowOverlayTracker {
    fn drop(&mut self) {
        self.close();
    }
}

struct WindowOverlayState {
    overlay: NativeOverlay,
    state_machine: OverlayStateMachine,
    app: AXUIElementRef,
    window: AXUIElementRef,
    observer: AXObserverRef,
    run_loop_source: CFRunLoopSourceRef,
    callback_refcon: *const Mutex<WindowOverlayState>,
    active: bool,
    sequence: u64,
    started_at: Instant,
}

unsafe impl Send for WindowOverlayState {}

impl WindowOverlayState {
    fn start(
        target: WindowOverlayTarget,
        style: OverlayStyle,
        _mtm: MainThreadMarker,
    ) -> Result<Arc<Mutex<WindowOverlayState>>> {
        let app = unsafe { AXUIElementCreateApplication(target.pid as c_int) };
        if app.is_null() {
            return Err(internal_error("failed to create AX application element"));
        }

        let window = match find_ax_window(app, &target) {
            Some(window) => window,
            None => {
                unsafe { CFRelease(app.cast()) };
                return Err(Error::new(
                    CaptureErrorCode::SourceUnavailable,
                    format!(
                        "failed to find AX window for shared window {}",
                        target.window_id
                    ),
                    true,
                ));
            }
        };

        let overlay = match NativeOverlay::new(style) {
            Ok(overlay) => overlay,
            Err(error) => {
                unsafe {
                    CFRelease(window.cast());
                    CFRelease(app.cast());
                }
                return Err(error);
            }
        };

        let mut observer = ptr::null();
        let error =
            unsafe { AXObserverCreate(target.pid as c_int, ax_observer_callback, &mut observer) };
        if error != AX_SUCCESS || observer.is_null() {
            unsafe {
                CFRelease(window.cast());
                CFRelease(app.cast());
            }
            return Err(internal_error(format!(
                "failed to create AXObserver for pid {}: {error}",
                target.pid
            )));
        }

        let run_loop_source = unsafe { AXObserverGetRunLoopSource(observer) };
        if run_loop_source.is_null() {
            unsafe {
                CFRelease(observer.cast());
                CFRelease(window.cast());
                CFRelease(app.cast());
            }
            return Err(internal_error("AXObserver returned a null run loop source"));
        }
        unsafe {
            CFRunLoopAddSource(CFRunLoopGetMain(), run_loop_source, kCFRunLoopDefaultMode);
        }

        let state = Arc::new(Mutex::new(WindowOverlayState {
            overlay,
            state_machine: OverlayStateMachine::default(),
            app,
            window,
            observer,
            run_loop_source,
            callback_refcon: ptr::null(),
            active: true,
            sequence: 0,
            started_at: Instant::now(),
        }));
        let callback_refcon = Arc::into_raw(Arc::clone(&state));
        {
            let mut state_guard = state
                .lock()
                .map_err(|_| internal_error("macOS window overlay lock was poisoned"))?;
            state_guard.callback_refcon = callback_refcon;
            if let Err(error) =
                state_guard.register_notifications(callback_refcon.cast_mut().cast())
            {
                state_guard.close();
                return Err(error);
            }
            state_guard.redraw_if_available();
        }

        Ok(state)
    }

    fn register_notifications(&mut self, refcon: *mut c_void) -> Result<()> {
        for notification in tracked_notifications() {
            let error = unsafe {
                AXObserverAddNotification(self.observer, self.window, notification, refcon)
            };
            if error != AX_SUCCESS {
                return Err(internal_error(format!(
                    "failed to register AX notification: {error}"
                )));
            }
        }
        Ok(())
    }

    fn handle_notification(&mut self, notification: CFStringRef) {
        if !self.active {
            return;
        }

        if cf_equal(notification, ax_ui_element_destroyed_notification()) {
            let action = self
                .state_machine
                .on_event(OverlayEvent::TargetUnavailable, self.elapsed());
            self.apply_action(action);
            return;
        }

        if cf_equal(notification, ax_window_miniaturized_notification()) {
            let action = self
                .state_machine
                .on_event(OverlayEvent::TargetHidden, self.elapsed());
            self.apply_action(action);
            return;
        }

        if cf_equal(notification, ax_window_deminiaturized_notification()) {
            self.redraw_if_available();
            return;
        }

        self.sequence = self.sequence.saturating_add(1);
        let sequence = self.sequence;
        let action = self
            .state_machine
            .on_event(OverlayEvent::TargetGeometryChanging, self.elapsed());
        self.apply_action(action);
        self.schedule_settle(sequence);
    }

    fn schedule_settle(&self, sequence: u64) {
        if self.callback_refcon.is_null() {
            return;
        }

        let refcon = self.callback_refcon as usize;
        unsafe {
            Arc::increment_strong_count(refcon as *const Mutex<WindowOverlayState>);
        }

        let when = DispatchTime::try_from(OVERLAY_SETTLE_DELAY)
            .unwrap_or_else(|_| DispatchTime::NOW.time(150_000_000));
        let _ = DispatchQueue::main().after(when, move || {
            let state_arc = unsafe { Arc::from_raw(refcon as *const Mutex<WindowOverlayState>) };
            if let Ok(mut state) = state_arc.lock() {
                if !state.active || state.sequence != sequence {
                    return;
                }
                let now = state.elapsed();
                let action = state.state_machine.on_tick(now);
                state.apply_action(action);
            };
        });
    }

    fn apply_action(&mut self, action: OverlayAction) {
        match action {
            OverlayAction::None => {}
            OverlayAction::Hide => self.overlay.hide(),
            OverlayAction::Show => self.redraw_if_available(),
        }
    }

    fn redraw_if_available(&mut self) {
        if !self.active || self.is_minimized() {
            let action = self
                .state_machine
                .on_event(OverlayEvent::TargetHidden, self.elapsed());
            self.apply_action(action);
            return;
        }

        let Some(rect) = ax_window_rect(self.window) else {
            let action = self
                .state_machine
                .on_event(OverlayEvent::TargetUnavailable, self.elapsed());
            self.apply_action(action);
            return;
        };

        if let Err(error) = self.overlay.show(rect) {
            eprintln!("[screen-capture] failed to show macOS window overlay: {error:?}");
            return;
        }
        let _ = self
            .state_machine
            .on_event(OverlayEvent::TargetVisible, self.elapsed());
    }

    fn is_minimized(&self) -> bool {
        copy_bool_attribute(self.window, ax_minimized_attribute()).unwrap_or(false)
    }

    fn hide(&mut self) {
        self.overlay.hide();
        let _ = self
            .state_machine
            .on_event(OverlayEvent::TargetHidden, self.elapsed());
    }

    fn close(&mut self) {
        if !self.active {
            return;
        }
        self.active = false;
        self.overlay.close();

        for notification in tracked_notifications() {
            unsafe {
                let _ = AXObserverRemoveNotification(self.observer, self.window, notification);
            }
        }
        unsafe {
            CFRunLoopRemoveSource(
                CFRunLoopGetMain(),
                self.run_loop_source,
                kCFRunLoopDefaultMode,
            );
            CFRelease(self.observer.cast());
            CFRelease(self.window.cast());
            CFRelease(self.app.cast());
        }

        if !self.callback_refcon.is_null() {
            unsafe {
                drop(Arc::from_raw(self.callback_refcon));
            }
            self.callback_refcon = ptr::null();
        }
    }

    fn elapsed(&self) -> Duration {
        self.started_at.elapsed()
    }
}

impl Drop for WindowOverlayState {
    fn drop(&mut self) {
        self.close();
    }
}

#[derive(Debug, Clone)]
struct OverlayStateMachine {
    visible: bool,
    settling_since: Option<Duration>,
    target_available: bool,
}

impl Default for OverlayStateMachine {
    fn default() -> Self {
        Self {
            visible: false,
            settling_since: None,
            target_available: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OverlayEvent {
    TargetGeometryChanging,
    TargetUnavailable,
    TargetVisible,
    TargetHidden,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OverlayAction {
    None,
    Hide,
    Show,
}

impl OverlayStateMachine {
    fn on_event(&mut self, event: OverlayEvent, now: Duration) -> OverlayAction {
        match event {
            OverlayEvent::TargetGeometryChanging => {
                self.target_available = true;
                self.settling_since = Some(now);
                self.hide()
            }
            OverlayEvent::TargetUnavailable | OverlayEvent::TargetHidden => {
                self.target_available = false;
                self.settling_since = None;
                self.hide()
            }
            OverlayEvent::TargetVisible => {
                self.target_available = true;
                if self.visible {
                    OverlayAction::None
                } else {
                    self.visible = true;
                    OverlayAction::Show
                }
            }
        }
    }

    fn on_tick(&mut self, now: Duration) -> OverlayAction {
        let Some(settling_since) = self.settling_since else {
            return OverlayAction::None;
        };
        if !self.target_available || now.saturating_sub(settling_since) < OVERLAY_SETTLE_DELAY {
            return OverlayAction::None;
        }

        self.settling_since = None;
        if self.visible {
            OverlayAction::None
        } else {
            self.visible = true;
            OverlayAction::Show
        }
    }

    fn hide(&mut self) -> OverlayAction {
        if self.visible {
            self.visible = false;
            OverlayAction::Hide
        } else {
            OverlayAction::None
        }
    }
}

extern "C" fn ax_observer_callback(
    _observer: AXObserverRef,
    _element: AXUIElementRef,
    notification: CFStringRef,
    refcon: *mut c_void,
) {
    if refcon.is_null() {
        return;
    }
    let state = unsafe { &*(refcon as *const Mutex<WindowOverlayState>) };
    if let Ok(mut state) = state.lock() {
        state.handle_notification(notification);
    }
}

fn tracked_notifications() -> [CFStringRef; 5] {
    [
        ax_moved_notification(),
        ax_resized_notification(),
        ax_ui_element_destroyed_notification(),
        ax_window_miniaturized_notification(),
        ax_window_deminiaturized_notification(),
    ]
}

fn find_ax_window(app: AXUIElementRef, target: &WindowOverlayTarget) -> Option<AXUIElementRef> {
    let mut windows_value = ptr::null();
    let error =
        unsafe { AXUIElementCopyAttributeValue(app, ax_windows_attribute(), &mut windows_value) };
    if error != AX_SUCCESS || windows_value.is_null() {
        return None;
    }

    let windows = windows_value as CFArrayRef;
    let count = unsafe { CFArrayGetCount(windows) };
    let mut fallback = None;
    for index in 0..count {
        let window = unsafe { CFArrayGetValueAtIndex(windows, index as CFIndex) as AXUIElementRef };
        if window.is_null() {
            continue;
        }

        if ax_window_id(window) == Some(target.window_id) {
            unsafe {
                CFRetain(window.cast());
                CFRelease(windows_value);
            }
            return Some(window);
        }

        if fallback.is_none() && ax_rect_matches_target(window, target) {
            unsafe { CFRetain(window.cast()) };
            fallback = Some(window);
        }
    }

    unsafe { CFRelease(windows_value) };
    fallback
}

fn ax_window_id(window: AXUIElementRef) -> Option<u32> {
    let mut window_id = 0;
    let error = unsafe { _AXUIElementGetWindow(window, &mut window_id) };
    (error == AX_SUCCESS).then_some(window_id)
}

fn ax_rect_matches_target(window: AXUIElementRef, target: &WindowOverlayTarget) -> bool {
    let Some(rect) = ax_window_rect(window) else {
        return false;
    };
    (rect.left - target.rect.left).abs() <= 2
        && (rect.top - target.rect.top).abs() <= 2
        && (rect.right - target.rect.right).abs() <= 2
        && (rect.bottom - target.rect.bottom).abs() <= 2
}

fn ax_window_rect(window: AXUIElementRef) -> Option<OverlayRect> {
    let position = copy_ax_point(window, ax_position_attribute())?;
    let size = copy_ax_size(window, ax_size_attribute())?;
    Some(overlay_rect_from_top_left(
        position.x,
        position.y,
        size.width,
        size.height,
    ))
}

fn copy_ax_point(element: AXUIElementRef, attribute: CFStringRef) -> Option<CGPoint> {
    let value = copy_attribute(element, attribute)?;
    let mut point = CGPoint { x: 0.0, y: 0.0 };
    let ok = unsafe {
        AXValueGetValue(
            value as AXValueRef,
            AX_VALUE_CGPOINT,
            (&mut point as *mut CGPoint).cast(),
        )
    };
    unsafe { CFRelease(value) };
    (ok != 0).then_some(point)
}

fn copy_ax_size(element: AXUIElementRef, attribute: CFStringRef) -> Option<CGSize> {
    let value = copy_attribute(element, attribute)?;
    let mut size = CGSize {
        width: 0.0,
        height: 0.0,
    };
    let ok = unsafe {
        AXValueGetValue(
            value as AXValueRef,
            AX_VALUE_CGSIZE,
            (&mut size as *mut CGSize).cast(),
        )
    };
    unsafe { CFRelease(value) };
    (ok != 0).then_some(size)
}

fn copy_bool_attribute(element: AXUIElementRef, attribute: CFStringRef) -> Option<bool> {
    let value = copy_attribute(element, attribute)?;
    let bool_value = unsafe { CFBooleanGetValue(value as CFBooleanRef) };
    unsafe { CFRelease(value) };
    Some(bool_value)
}

fn copy_attribute(element: AXUIElementRef, attribute: CFStringRef) -> Option<CFTypeRef> {
    let mut value = ptr::null();
    let error = unsafe { AXUIElementCopyAttributeValue(element, attribute, &mut value) };
    (error == AX_SUCCESS && !value.is_null()).then_some(value)
}

fn accessibility_is_trusted() -> bool {
    macos_accessibility_client::accessibility::application_is_trusted()
}

fn request_accessibility_prompt() -> bool {
    macos_accessibility_client::accessibility::application_is_trusted_with_prompt()
}

fn cf_equal(left: CFStringRef, right: CFStringRef) -> bool {
    unsafe { CFEqual(left.cast(), right.cast()) != 0 }
}

fn ax_windows_attribute() -> CFStringRef {
    static VALUE: OnceLock<usize> = OnceLock::new();
    ax_static_string("AXWindows", &VALUE)
}

fn ax_position_attribute() -> CFStringRef {
    static VALUE: OnceLock<usize> = OnceLock::new();
    ax_static_string("AXPosition", &VALUE)
}

fn ax_size_attribute() -> CFStringRef {
    static VALUE: OnceLock<usize> = OnceLock::new();
    ax_static_string("AXSize", &VALUE)
}

fn ax_minimized_attribute() -> CFStringRef {
    static VALUE: OnceLock<usize> = OnceLock::new();
    ax_static_string("AXMinimized", &VALUE)
}

fn ax_moved_notification() -> CFStringRef {
    static VALUE: OnceLock<usize> = OnceLock::new();
    ax_static_string("AXMoved", &VALUE)
}

fn ax_resized_notification() -> CFStringRef {
    static VALUE: OnceLock<usize> = OnceLock::new();
    ax_static_string("AXResized", &VALUE)
}

fn ax_ui_element_destroyed_notification() -> CFStringRef {
    static VALUE: OnceLock<usize> = OnceLock::new();
    ax_static_string("AXUIElementDestroyed", &VALUE)
}

fn ax_window_miniaturized_notification() -> CFStringRef {
    static VALUE: OnceLock<usize> = OnceLock::new();
    ax_static_string("AXWindowMiniaturized", &VALUE)
}

fn ax_window_deminiaturized_notification() -> CFStringRef {
    static VALUE: OnceLock<usize> = OnceLock::new();
    ax_static_string("AXWindowDeminiaturized", &VALUE)
}

fn ax_static_string(value: &'static str, slot: &'static OnceLock<usize>) -> CFStringRef {
    *slot.get_or_init(|| {
        let value = CString::new(value).expect("AX constant names do not contain null bytes");
        unsafe {
            CFStringCreateWithCString(kCFAllocatorDefault, value.as_ptr(), kCFStringEncodingUTF8)
                as usize
        }
    }) as CFStringRef
}

fn overlay_rect_from_ns_rect(rect: NSRect) -> OverlayRect {
    OverlayRect {
        left: round_i32(rect.origin.x),
        top: round_i32(rect.origin.y),
        right: round_i32(rect.origin.x + rect.size.width),
        bottom: round_i32(rect.origin.y + rect.size.height),
    }
}

fn overlay_rect_from_top_left(x: f64, y: f64, width: f64, height: f64) -> OverlayRect {
    let max_y = max_screen_y();
    let bottom_left_y = max_y - y - height;
    OverlayRect {
        left: round_i32(x),
        top: round_i32(bottom_left_y),
        right: round_i32(x + width),
        bottom: round_i32(bottom_left_y + height),
    }
}

fn max_screen_y() -> f64 {
    run_on_main(|mtm| {
        let screens = NSScreen::screens(mtm);
        let mut max_y = 0.0_f64;
        for index in 0..screens.count() {
            let frame = screens.objectAtIndex(index).frame();
            max_y = max_y.max(frame.origin.y + frame.size.height);
        }
        max_y
    })
}

fn parse_source_id(source_id: &str, expected_kind: &str) -> Result<u32> {
    let raw_id = source_id
        .strip_prefix(expected_kind)
        .and_then(|suffix| suffix.strip_prefix(':'))
        .ok_or_else(|| source_not_found(source_id))?;
    raw_id
        .parse::<u32>()
        .map_err(|_| source_not_found(source_id))
}

fn source_not_found(source_id: &str) -> Error {
    Error::new(
        CaptureErrorCode::SourceNotFound,
        format!("capture source was not found: {source_id}"),
        true,
    )
}

fn round_i32(value: f64) -> i32 {
    if value.is_finite() {
        value
            .round()
            .clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32
    } else {
        0
    }
}

fn internal_error(message: impl Into<String>) -> Error {
    Error::new(CaptureErrorCode::Internal, message.into(), false)
}
