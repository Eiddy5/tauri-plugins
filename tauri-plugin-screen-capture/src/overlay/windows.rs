use std::{
    collections::HashMap,
    ffi::{c_void, OsStr},
    marker::PhantomData,
    os::windows::ffi::OsStrExt,
    rc::Rc,
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc, Mutex, OnceLock,
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use async_trait::async_trait;
use windows::{
    core::{Error as WindowsError, PCWSTR},
    Win32::{
        Foundation::{
            GetLastError, COLORREF, ERROR_CLASS_ALREADY_EXISTS, HWND, LPARAM, LRESULT, RECT, WPARAM,
        },
        Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_EXTENDED_FRAME_BOUNDS},
        Graphics::Gdi::{
            CreateSolidBrush, DeleteObject, GetMonitorInfoW, HGDIOBJ, HMONITOR, MONITORINFO,
        },
        UI::Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK},
        UI::WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetWindowRect,
            GetWindowThreadProcessId, IsIconic, IsWindow, IsWindowVisible, PeekMessageW,
            RegisterClassW, SetLayeredWindowAttributes, SetWindowDisplayAffinity, SetWindowPos,
            ShowWindow, TranslateMessage, CS_HREDRAW, CS_VREDRAW, EVENT_OBJECT_DESTROY,
            EVENT_OBJECT_HIDE, EVENT_OBJECT_LOCATIONCHANGE, EVENT_OBJECT_SHOW,
            EVENT_SYSTEM_MOVESIZEEND, EVENT_SYSTEM_MOVESIZESTART, HMENU, HWND_TOPMOST, LWA_ALPHA,
            MSG, OBJID_WINDOW, PM_REMOVE, SWP_NOACTIVATE, SWP_NOOWNERZORDER, SWP_NOZORDER, SW_HIDE,
            SW_SHOWNOACTIVATE, WDA_EXCLUDEFROMCAPTURE, WINEVENT_OUTOFCONTEXT, WM_QUIT, WNDCLASSW,
            WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT,
            WS_POPUP,
        },
    },
};
use windows_capture::monitor::Monitor;

use crate::{
    error::Error,
    models::{CaptureErrorCode, CaptureSourceKind},
    overlay::{corner_segments, OverlayRect, OverlayStyle, OverlayTarget, ShareOverlay},
    Result,
};

const CLASS_NAME: &str = "TauriPluginScreenCaptureShareBorder";
const COMMAND_POLL_INTERVAL: Duration = Duration::from_millis(16);
static NEXT_WINDOW_EVENT_REGISTRATION_ID: AtomicU64 = AtomicU64::new(1);
static WINDOW_EVENT_REGISTRY: OnceLock<Mutex<WindowEventRegistry>> = OnceLock::new();

#[derive(Debug)]
pub struct WindowsShareOverlay {
    style: OverlayStyle,
    host: Mutex<Option<OverlayHost>>,
    window_event_registration_id: u64,
}

impl Default for WindowsShareOverlay {
    fn default() -> Self {
        Self {
            style: OverlayStyle::default(),
            host: Mutex::new(None),
            window_event_registration_id: next_window_event_registration_id(),
        }
    }
}

#[async_trait]
impl ShareOverlay for WindowsShareOverlay {
    async fn start(&self, target: OverlayTarget) -> Result<()> {
        match target.source_kind {
            CaptureSourceKind::Window => {
                let target_hwnd = match windows_target_handle_from_source_id(&target.source_id) {
                    Some(target_hwnd) => target_hwnd,
                    None => {
                        self.clear_window_event_hooks().await?;
                        self.hide().await?;
                        return Ok(());
                    }
                };
                let sender = self.ensure_host_sender()?;
                let rect = match window_bounds_from_source_id(&target.source_id) {
                    Ok(rect) => rect,
                    Err(_) => {
                        self.clear_window_event_hooks().await?;
                        send_overlay_command(sender, OverlayCommand::hide).await?;
                        return Ok(());
                    }
                };
                let source_id = target.source_id.clone();
                let style = self.style;
                let registration_id = self.window_event_registration_id;
                let event_sender = sender.clone();

                send_overlay_command(sender.clone(), move |reply| {
                    OverlayCommand::RegisterWindowEventTarget {
                        registration_id,
                        target_hwnd,
                        source_id,
                        style,
                        event_sender,
                        reply,
                    }
                })
                .await?;

                match rect {
                    Some(rect) => {
                        let style = self.style;
                        let placement = OverlayPlacement::for_window(target_hwnd);
                        send_overlay_command(sender, move |reply| OverlayCommand::ShowRect {
                            rect,
                            style,
                            placement,
                            reply,
                        })
                        .await
                    }
                    None => {
                        send_overlay_command(sender, OverlayCommand::hide).await?;
                        Ok(())
                    }
                }
            }
            CaptureSourceKind::Display => {
                self.clear_window_event_hooks().await?;
                match display_bounds_from_source_id(&target.source_id) {
                    Some(rect) => self.show_rect(rect).await,
                    None => {
                        self.hide().await?;
                        Ok(())
                    }
                }
            }
        }
    }

    async fn show(&self) -> Result<()> {
        if let Some(sender) = self.host_sender()? {
            send_overlay_command(sender, OverlayCommand::show).await?;
        }

        Ok(())
    }

    async fn hide(&self) -> Result<()> {
        if let Some(sender) = self.host_sender()? {
            send_overlay_command(sender, OverlayCommand::hide).await?;
        }

        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        if let Some(host) = self.take_host()? {
            host.shutdown().await?;
        }

        Ok(())
    }
}

impl WindowsShareOverlay {
    async fn show_rect(&self, rect: OverlayRect) -> Result<()> {
        let sender = self.ensure_host_sender()?;
        let style = self.style;
        let placement = OverlayPlacement::for_display();

        send_overlay_command(sender, move |reply| OverlayCommand::ShowRect {
            rect,
            style,
            placement,
            reply,
        })
        .await
    }

    fn ensure_host_sender(&self) -> Result<mpsc::Sender<OverlayCommand>> {
        let mut host = self.lock_host()?;

        if host.is_none() {
            *host = Some(OverlayHost::spawn()?);
        }

        Ok(host
            .as_ref()
            .expect("overlay host must exist after initialization")
            .sender
            .clone())
    }

    fn host_sender(&self) -> Result<Option<mpsc::Sender<OverlayCommand>>> {
        Ok(self.lock_host()?.as_ref().map(|host| host.sender.clone()))
    }

    fn take_host(&self) -> Result<Option<OverlayHost>> {
        Ok(self.lock_host()?.take())
    }

    fn lock_host(&self) -> Result<std::sync::MutexGuard<'_, Option<OverlayHost>>> {
        self.host.lock().map_err(|_| {
            Error::new(
                CaptureErrorCode::Internal,
                "share border overlay host state lock was poisoned",
                false,
            )
        })
    }

    async fn clear_window_event_hooks(&self) -> Result<()> {
        if let Some(sender) = self.host_sender()? {
            let registration_id = self.window_event_registration_id;
            send_overlay_command(sender, move |reply| {
                OverlayCommand::ClearWindowEventTarget {
                    registration_id,
                    reply,
                }
            })
            .await?;
        }

        Ok(())
    }
}

pub fn windows_target_handle_from_source_id(source_id: &str) -> Option<usize> {
    source_id
        .strip_prefix("window:")
        .and_then(|id| usize::from_str_radix(id, 16).ok())
}

pub fn windows_display_index_from_source_id(source_id: &str) -> Option<usize> {
    let display_id = source_id.strip_prefix("display:")?;
    let index = display_id
        .rsplit_once(':')
        .map(|(_, index)| index)
        .unwrap_or(display_id);

    index
        .parse::<usize>()
        .ok()
        .and_then(|index| index.checked_sub(1))
}

fn display_bounds_from_source_id(source_id: &str) -> Option<OverlayRect> {
    let Some(index) = windows_display_index_from_source_id(source_id) else {
        return None;
    };
    let monitor = match Monitor::from_index(index + 1) {
        Ok(monitor) => monitor,
        Err(_) => return None,
    };
    let mut info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    let ok = unsafe { GetMonitorInfoW(HMONITOR(monitor.as_raw_hmonitor()), &mut info).as_bool() };
    if !ok {
        return None;
    }

    Some(OverlayRect {
        left: info.rcMonitor.left,
        top: info.rcMonitor.top,
        right: info.rcMonitor.right,
        bottom: info.rcMonitor.bottom,
    })
}

fn window_bounds_from_source_id(source_id: &str) -> Result<Option<OverlayRect>> {
    let handle = windows_target_handle_from_source_id(source_id).ok_or_else(|| {
        Error::new(
            CaptureErrorCode::SourceNotFound,
            format!("invalid Windows window source id: {source_id}"),
            false,
        )
    })?;
    let hwnd = HWND(handle as *mut c_void);

    if !unsafe { IsWindow(Some(hwnd)).as_bool() } {
        return Err(Error::new(
            CaptureErrorCode::SourceUnavailable,
            format!("Windows window source is no longer available: {source_id}"),
            true,
        ));
    }

    if unsafe { IsIconic(hwnd).as_bool() } || !unsafe { IsWindowVisible(hwnd).as_bool() } {
        return Ok(None);
    }

    let mut window_rect = RECT::default();
    unsafe { GetWindowRect(hwnd, &mut window_rect) }.map_err(|error| {
        Error::new(
            CaptureErrorCode::SourceUnavailable,
            format!("failed to resolve Windows window bounds for {source_id}: {error}"),
            true,
        )
    })?;

    Ok(Some(preferred_window_bounds(
        rect_to_overlay_rect(window_rect),
        dwm_extended_frame_bounds(hwnd),
    )))
}

fn dwm_extended_frame_bounds(hwnd: HWND) -> Option<OverlayRect> {
    let mut rect = RECT::default();
    unsafe {
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            &mut rect as *mut _ as *mut c_void,
            std::mem::size_of::<RECT>() as u32,
        )
    }
    .ok()?;

    Some(rect_to_overlay_rect(rect))
}

fn rect_to_overlay_rect(rect: RECT) -> OverlayRect {
    OverlayRect {
        left: rect.left,
        top: rect.top,
        right: rect.right,
        bottom: rect.bottom,
    }
}

fn preferred_window_bounds(
    get_window_rect: OverlayRect,
    dwm_frame_bounds: Option<OverlayRect>,
) -> OverlayRect {
    dwm_frame_bounds.unwrap_or(get_window_rect)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct OverlayPlacement {
    owner_hwnd: Option<usize>,
    topmost: bool,
}

impl OverlayPlacement {
    const fn for_window(owner_hwnd: usize) -> Self {
        Self {
            owner_hwnd: Some(owner_hwnd),
            topmost: false,
        }
    }

    const fn for_display() -> Self {
        Self {
            owner_hwnd: None,
            topmost: true,
        }
    }
}

#[derive(Debug)]
struct WindowEventHooks {
    registration_id: u64,
    handles: Vec<usize>,
}

impl WindowEventHooks {
    fn unregister(self) {
        unregister_window_event_target(self.registration_id);

        for handle in self.handles {
            unsafe {
                let _ = UnhookWinEvent(HWINEVENTHOOK(handle as *mut c_void));
            }
        }
    }
}

#[derive(Default)]
struct WindowEventRegistry {
    targets_by_hwnd: HashMap<usize, Vec<WindowEventTarget>>,
    hook_targets: HashMap<usize, WindowEventHookTarget>,
}

#[derive(Clone)]
struct WindowEventHookTarget {
    registration_id: u64,
    target_hwnd: usize,
}

#[derive(Clone)]
struct WindowEventTarget {
    registration_id: u64,
    source_id: String,
    style: OverlayStyle,
    sender: mpsc::Sender<OverlayCommand>,
    moving_or_sizing: bool,
}

enum WindowEventAction {
    Hide {
        sender: mpsc::Sender<OverlayCommand>,
    },
    HideAndClear {
        sender: mpsc::Sender<OverlayCommand>,
        registration_id: u64,
    },
    RefreshBounds {
        source_id: String,
        style: OverlayStyle,
        target_hwnd: usize,
        sender: mpsc::Sender<OverlayCommand>,
    },
}

fn next_window_event_registration_id() -> u64 {
    NEXT_WINDOW_EVENT_REGISTRATION_ID.fetch_add(1, Ordering::Relaxed)
}

fn window_event_registry() -> &'static Mutex<WindowEventRegistry> {
    WINDOW_EVENT_REGISTRY.get_or_init(|| Mutex::new(WindowEventRegistry::default()))
}

fn register_window_event_target(
    registration_id: u64,
    target_hwnd: usize,
    source_id: String,
    style: OverlayStyle,
    sender: mpsc::Sender<OverlayCommand>,
) -> Result<()> {
    let mut registry = lock_window_event_registry()?;
    let targets = registry.targets_by_hwnd.entry(target_hwnd).or_default();
    targets.retain(|target| target.registration_id != registration_id);
    targets.push(WindowEventTarget {
        registration_id,
        source_id,
        style,
        sender,
        moving_or_sizing: false,
    });

    Ok(())
}

fn register_window_event_hook_handles(
    registration_id: u64,
    target_hwnd: usize,
    handles: &[usize],
) -> Result<()> {
    let mut registry = lock_window_event_registry()?;
    registry
        .hook_targets
        .retain(|_, target| target.registration_id != registration_id);

    for handle in handles {
        registry.hook_targets.insert(
            *handle,
            WindowEventHookTarget {
                registration_id,
                target_hwnd,
            },
        );
    }

    Ok(())
}

fn unregister_window_event_target(registration_id: u64) {
    let Ok(mut registry) = window_event_registry().lock() else {
        return;
    };

    let mut empty_hwnds = Vec::new();
    for (target_hwnd, targets) in &mut registry.targets_by_hwnd {
        targets.retain(|target| target.registration_id != registration_id);
        if targets.is_empty() {
            empty_hwnds.push(*target_hwnd);
        }
    }

    for target_hwnd in empty_hwnds {
        registry.targets_by_hwnd.remove(&target_hwnd);
    }

    registry
        .hook_targets
        .retain(|_, target| target.registration_id != registration_id);
}

fn lock_window_event_registry() -> Result<std::sync::MutexGuard<'static, WindowEventRegistry>> {
    window_event_registry().lock().map_err(|_| {
        Error::new(
            CaptureErrorCode::Internal,
            "share border overlay WinEvent registry lock was poisoned",
            false,
        )
    })
}

fn install_window_event_hooks(process_id: u32, thread_id: u32) -> Result<Vec<usize>> {
    let mut handles = Vec::with_capacity(2);

    for (event_min, event_max) in [
        (EVENT_SYSTEM_MOVESIZESTART, EVENT_SYSTEM_MOVESIZEEND),
        (EVENT_OBJECT_DESTROY, EVENT_OBJECT_LOCATIONCHANGE),
    ] {
        match install_window_event_hook(event_min, event_max, process_id, thread_id) {
            Ok(handle) => handles.push(handle),
            Err(error) => {
                for handle in handles {
                    unsafe {
                        let _ = UnhookWinEvent(HWINEVENTHOOK(handle as *mut c_void));
                    }
                }
                return Err(error);
            }
        }
    }

    Ok(handles)
}

fn install_window_event_hook(
    event_min: u32,
    event_max: u32,
    process_id: u32,
    thread_id: u32,
) -> Result<usize> {
    let hook = unsafe {
        SetWinEventHook(
            event_min,
            event_max,
            None,
            Some(win_event_proc),
            process_id,
            thread_id,
            WINEVENT_OUTOFCONTEXT,
        )
    };

    if hook.is_invalid() {
        return Err(Error::new(
            CaptureErrorCode::Internal,
            format!(
                "failed to install Windows share border WinEvent hook for event range {event_min}..={event_max}: {:?}",
                unsafe { GetLastError() }
            ),
            true,
        ));
    }

    Ok(hook.0 as usize)
}

fn target_window_thread(hwnd: HWND) -> Option<(u32, u32)> {
    if !unsafe { IsWindow(Some(hwnd)).as_bool() } {
        return None;
    }

    let mut process_id = 0;
    let thread_id = unsafe { GetWindowThreadProcessId(hwnd, Some(&mut process_id)) };
    if thread_id == 0 {
        return None;
    }

    Some((process_id, thread_id))
}

unsafe extern "system" fn win_event_proc(
    hook: HWINEVENTHOOK,
    event: u32,
    hwnd: HWND,
    object_id: i32,
    _child_id: i32,
    _event_thread: u32,
    _event_time: u32,
) {
    dispatch_window_event(hook.0 as usize, event, hwnd.0 as usize, object_id);
}

fn dispatch_window_event(hook: usize, event: u32, hwnd: usize, object_id: i32) {
    if object_id != OBJID_WINDOW.0 {
        return;
    }

    let action = {
        let Ok(mut registry) = window_event_registry().lock() else {
            return;
        };
        let Some(hook_target) = registry.hook_targets.get(&hook).cloned() else {
            return;
        };
        if hook_target.target_hwnd != hwnd {
            return;
        }

        let Some(targets) = registry.targets_by_hwnd.get_mut(&hwnd) else {
            return;
        };
        let Some(target_index) = targets
            .iter()
            .position(|target| target.registration_id == hook_target.registration_id)
        else {
            return;
        };

        match event {
            EVENT_SYSTEM_MOVESIZESTART => {
                let target = &mut targets[target_index];
                target.moving_or_sizing = true;
                Some(WindowEventAction::Hide {
                    sender: target.sender.clone(),
                })
            }
            EVENT_SYSTEM_MOVESIZEEND => {
                let target = &mut targets[target_index];
                target.moving_or_sizing = false;
                Some(WindowEventAction::RefreshBounds {
                    source_id: target.source_id.clone(),
                    style: target.style,
                    target_hwnd: hwnd,
                    sender: target.sender.clone(),
                })
            }
            EVENT_OBJECT_HIDE => {
                let target = &mut targets[target_index];
                target.moving_or_sizing = false;
                Some(WindowEventAction::Hide {
                    sender: target.sender.clone(),
                })
            }
            EVENT_OBJECT_DESTROY => {
                let target = targets.remove(target_index);
                if targets.is_empty() {
                    registry.targets_by_hwnd.remove(&hwnd);
                }
                registry
                    .hook_targets
                    .retain(|_, existing| existing.registration_id != hook_target.registration_id);
                Some(WindowEventAction::HideAndClear {
                    sender: target.sender,
                    registration_id: target.registration_id,
                })
            }
            EVENT_OBJECT_SHOW | EVENT_OBJECT_LOCATIONCHANGE => {
                let target = &targets[target_index];
                if target.moving_or_sizing {
                    None
                } else {
                    Some(WindowEventAction::RefreshBounds {
                        source_id: target.source_id.clone(),
                        style: target.style,
                        target_hwnd: hwnd,
                        sender: target.sender.clone(),
                    })
                }
            }
            _ => None,
        }
    };

    if let Some(action) = action {
        match action {
            WindowEventAction::Hide { sender } => {
                let _ = sender.send(OverlayCommand::HideForWindowEvent);
            }
            WindowEventAction::HideAndClear {
                sender,
                registration_id,
            } => {
                let _ = sender.send(OverlayCommand::HideForWindowEvent);
                let _ = sender
                    .send(OverlayCommand::ClearWindowEventTargetForWindowEvent { registration_id });
            }
            WindowEventAction::RefreshBounds {
                source_id,
                style,
                target_hwnd,
                sender,
            } => {
                let command = match window_bounds_from_source_id(&source_id) {
                    Ok(Some(rect)) => OverlayCommand::ShowRectForWindowEvent {
                        rect,
                        style,
                        placement: OverlayPlacement::for_window(target_hwnd),
                    },
                    Ok(None) | Err(_) => OverlayCommand::HideForWindowEvent,
                };
                let _ = sender.send(command);
            }
        }
    }
}

#[derive(Debug)]
struct OverlayHost {
    sender: mpsc::Sender<OverlayCommand>,
    thread: Option<JoinHandle<()>>,
}

impl OverlayHost {
    fn spawn() -> Result<Self> {
        let (sender, receiver) = mpsc::channel();
        let thread = thread::Builder::new()
            .name("tauri-screen-capture-share-overlay".into())
            .spawn(move || OverlayThread::default().run(receiver))
            .map_err(|error| {
                Error::new(
                    CaptureErrorCode::Internal,
                    format!("failed to start share border overlay UI thread: {error}"),
                    true,
                )
            })?;

        Ok(Self {
            sender,
            thread: Some(thread),
        })
    }

    async fn shutdown(self) -> Result<()> {
        tokio::task::spawn_blocking(move || {
            let mut host = self;
            host.shutdown_blocking()
        })
        .await
        .map_err(|error| {
            Error::new(
                CaptureErrorCode::Internal,
                format!("failed to join share border overlay shutdown task: {error}"),
                false,
            )
        })?
    }

    fn shutdown_blocking(&mut self) -> Result<()> {
        let result = self.request_blocking(OverlayCommand::shutdown);
        let join_result = self.join_thread();

        result.and(join_result)
    }

    fn request_blocking(
        &self,
        build_command: impl FnOnce(OverlayReply) -> OverlayCommand,
    ) -> Result<()> {
        let (reply, response) = mpsc::channel();

        self.sender.send(build_command(reply)).map_err(|_| {
            Error::new(
                CaptureErrorCode::Internal,
                "share border overlay UI thread stopped before receiving command",
                true,
            )
        })?;

        response.recv().map_err(|_| {
            Error::new(
                CaptureErrorCode::Internal,
                "share border overlay UI thread stopped before completing command",
                true,
            )
        })?
    }

    fn join_thread(&mut self) -> Result<()> {
        if let Some(thread) = self.thread.take() {
            thread.join().map_err(|_| {
                Error::new(
                    CaptureErrorCode::Internal,
                    "share border overlay UI thread panicked",
                    false,
                )
            })?;
        }

        Ok(())
    }
}

impl Drop for OverlayHost {
    fn drop(&mut self) {
        if self.thread.is_some() {
            let _ = self.shutdown_blocking();
        }
    }
}

type OverlayReply = mpsc::Sender<Result<()>>;

enum OverlayCommand {
    ShowRect {
        rect: OverlayRect,
        style: OverlayStyle,
        placement: OverlayPlacement,
        reply: OverlayReply,
    },
    Show {
        reply: OverlayReply,
    },
    Hide {
        reply: OverlayReply,
    },
    ShowRectForWindowEvent {
        rect: OverlayRect,
        style: OverlayStyle,
        placement: OverlayPlacement,
    },
    HideForWindowEvent,
    RegisterWindowEventTarget {
        registration_id: u64,
        target_hwnd: usize,
        source_id: String,
        style: OverlayStyle,
        event_sender: mpsc::Sender<OverlayCommand>,
        reply: OverlayReply,
    },
    ClearWindowEventTarget {
        registration_id: u64,
        reply: OverlayReply,
    },
    ClearWindowEventTargetForWindowEvent {
        registration_id: u64,
    },
    Shutdown {
        reply: OverlayReply,
    },
}

impl OverlayCommand {
    fn show(reply: OverlayReply) -> Self {
        Self::Show { reply }
    }

    fn hide(reply: OverlayReply) -> Self {
        Self::Hide { reply }
    }

    fn shutdown(reply: OverlayReply) -> Self {
        Self::Shutdown { reply }
    }
}

async fn send_overlay_command(
    sender: mpsc::Sender<OverlayCommand>,
    build_command: impl FnOnce(OverlayReply) -> OverlayCommand + Send + 'static,
) -> Result<()> {
    tokio::task::spawn_blocking(move || {
        let (reply, response) = mpsc::channel();

        sender.send(build_command(reply)).map_err(|_| {
            Error::new(
                CaptureErrorCode::Internal,
                "share border overlay UI thread stopped before receiving command",
                true,
            )
        })?;

        response.recv().map_err(|_| {
            Error::new(
                CaptureErrorCode::Internal,
                "share border overlay UI thread stopped before completing command",
                true,
            )
        })?
    })
    .await
    .map_err(|error| {
        Error::new(
            CaptureErrorCode::Internal,
            format!("failed to wait for share border overlay command: {error}"),
            false,
        )
    })?
}

#[derive(Default)]
struct OverlayThread {
    windows: Vec<OverlayWindow>,
    window_event_hooks: HashMap<u64, WindowEventHooks>,
}

impl OverlayThread {
    fn run(mut self, receiver: mpsc::Receiver<OverlayCommand>) {
        loop {
            if !pump_window_messages() {
                break;
            }

            match receiver.recv_timeout(COMMAND_POLL_INTERVAL) {
                Ok(command) => {
                    if self.handle_command(command) {
                        break;
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }

        self.clear_all_window_event_targets();
        self.windows.clear();
    }

    fn handle_command(&mut self, command: OverlayCommand) -> bool {
        match command {
            OverlayCommand::ShowRect {
                rect,
                style,
                placement,
                reply,
            } => {
                send_reply(reply, self.show_rect(rect, style, placement));
                false
            }
            OverlayCommand::Show { reply } => {
                for window in &self.windows {
                    window.show();
                }
                send_reply(reply, Ok(()));
                false
            }
            OverlayCommand::Hide { reply } => {
                for window in &self.windows {
                    window.hide();
                }
                send_reply(reply, Ok(()));
                false
            }
            OverlayCommand::ShowRectForWindowEvent {
                rect,
                style,
                placement,
            } => {
                if let Err(error) = self.show_rect(rect, style, placement) {
                    eprintln!(
                        "[screen-capture] failed to refresh share overlay from WinEvent: {error}"
                    );
                }
                false
            }
            OverlayCommand::HideForWindowEvent => {
                for window in &self.windows {
                    window.hide();
                }
                false
            }
            OverlayCommand::RegisterWindowEventTarget {
                registration_id,
                target_hwnd,
                source_id,
                style,
                event_sender,
                reply,
            } => {
                send_reply(
                    reply,
                    self.register_window_event_target(
                        registration_id,
                        target_hwnd,
                        source_id,
                        style,
                        event_sender,
                    ),
                );
                false
            }
            OverlayCommand::ClearWindowEventTarget {
                registration_id,
                reply,
            } => {
                self.clear_window_event_target(registration_id);
                send_reply(reply, Ok(()));
                false
            }
            OverlayCommand::ClearWindowEventTargetForWindowEvent { registration_id } => {
                self.clear_window_event_target(registration_id);
                false
            }
            OverlayCommand::Shutdown { reply } => {
                self.clear_all_window_event_targets();
                self.windows.clear();
                send_reply(reply, Ok(()));
                true
            }
        }
    }

    fn show_rect(
        &mut self,
        rect: OverlayRect,
        style: OverlayStyle,
        placement: OverlayPlacement,
    ) -> Result<()> {
        let segments = corner_segments(rect, style);

        if self.windows.len() != segments.len()
            || self
                .windows
                .first()
                .is_some_and(|window| window.placement != placement)
        {
            let mut windows = Vec::with_capacity(segments.len());

            for segment in &segments {
                let window = OverlayWindow::new(style.color, placement)?;
                window.position(segment.x, segment.y, segment.width, segment.height)?;
                window.show();
                windows.push(window);
            }

            self.windows = windows;
            return Ok(());
        }

        for (window, segment) in self.windows.iter().zip(segments) {
            window.position(segment.x, segment.y, segment.width, segment.height)?;
            window.show();
        }

        Ok(())
    }

    fn register_window_event_target(
        &mut self,
        registration_id: u64,
        target_hwnd: usize,
        source_id: String,
        style: OverlayStyle,
        event_sender: mpsc::Sender<OverlayCommand>,
    ) -> Result<()> {
        self.clear_window_event_target(registration_id);

        let hwnd = HWND(target_hwnd as *mut c_void);
        let Some((process_id, thread_id)) = target_window_thread(hwnd) else {
            unregister_window_event_target(registration_id);
            for window in &self.windows {
                window.hide();
            }
            return Ok(());
        };

        let handles = install_window_event_hooks(process_id, thread_id)?;
        let hooks = WindowEventHooks {
            registration_id,
            handles,
        };

        if let Err(error) = register_window_event_target(
            registration_id,
            target_hwnd,
            source_id,
            style,
            event_sender,
        )
        .and_then(|_| {
            register_window_event_hook_handles(registration_id, target_hwnd, &hooks.handles)
        }) {
            hooks.unregister();
            return Err(error);
        }

        self.window_event_hooks.insert(registration_id, hooks);
        Ok(())
    }

    fn clear_window_event_target(&mut self, registration_id: u64) {
        if let Some(hooks) = self.window_event_hooks.remove(&registration_id) {
            hooks.unregister();
        } else {
            unregister_window_event_target(registration_id);
        }
    }

    fn clear_all_window_event_targets(&mut self) {
        let hooks = std::mem::take(&mut self.window_event_hooks);
        for (_, hooks) in hooks {
            hooks.unregister();
        }
    }
}

fn pump_window_messages() -> bool {
    let mut message = MSG::default();

    while unsafe { PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() } {
        if message.message == WM_QUIT {
            return false;
        }

        unsafe {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }

    true
}

fn send_reply(reply: OverlayReply, result: Result<()>) {
    let _ = reply.send(result);
}

#[derive(Debug)]
struct OverlayWindow {
    hwnd: HWND,
    placement: OverlayPlacement,
    _ui_thread: PhantomData<Rc<()>>,
}

impl OverlayWindow {
    fn new(color: u32, placement: OverlayPlacement) -> Result<Self> {
        register_window_class(color)?;

        let class_name = wide(CLASS_NAME);
        let ex_style = WS_EX_TOOLWINDOW
            | WS_EX_TRANSPARENT
            | WS_EX_LAYERED
            | WS_EX_NOACTIVATE
            | if placement.topmost {
                WS_EX_TOPMOST
            } else {
                Default::default()
            };
        let owner = placement.owner_hwnd.map(|owner| HWND(owner as *mut c_void));
        let hwnd = unsafe {
            CreateWindowExW(
                ex_style,
                PCWSTR(class_name.as_ptr()),
                PCWSTR(class_name.as_ptr()),
                WS_POPUP,
                0,
                0,
                1,
                1,
                owner,
                Some(HMENU::default()),
                None,
                None,
            )
        }
        .map_err(|error| {
            Error::new(
                CaptureErrorCode::Internal,
                format!("failed to create share border overlay window: {error}"),
                true,
            )
        })?;

        let window = Self {
            hwnd,
            placement,
            _ui_thread: PhantomData,
        };
        window.configure()?;

        Ok(window)
    }

    fn configure(&self) -> Result<()> {
        unsafe { SetLayeredWindowAttributes(self.hwnd, COLORREF(0), 255, LWA_ALPHA) }.map_err(
            |error| {
                windows_error(
                    "failed to set share border overlay layered window attributes",
                    error,
                )
            },
        )?;

        unsafe { SetWindowDisplayAffinity(self.hwnd, WDA_EXCLUDEFROMCAPTURE) }.map_err(
            |error| {
                windows_error(
                    "failed to exclude share border overlay window from capture with SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE)",
                    error,
                )
            },
        )?;

        Ok(())
    }

    fn position(&self, x: i32, y: i32, width: i32, height: i32) -> Result<()> {
        let (insert_after, flags) = if self.placement.topmost {
            (Some(HWND_TOPMOST), SWP_NOACTIVATE | SWP_NOOWNERZORDER)
        } else {
            (None, SWP_NOACTIVATE | SWP_NOOWNERZORDER | SWP_NOZORDER)
        };

        unsafe {
            SetWindowPos(self.hwnd, insert_after, x, y, width, height, flags).map_err(|error| {
                windows_error(
                    "failed to position share border overlay window with SetWindowPos",
                    error,
                )
            })?;
        }

        Ok(())
    }

    fn show(&self) {
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
        }
    }

    fn hide(&self) {
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_HIDE);
        }
    }
}

impl Drop for OverlayWindow {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

fn register_window_class(color: u32) -> Result<()> {
    let brush = unsafe { CreateSolidBrush(COLORREF(rgb_to_colorref(color))) };

    if brush.is_invalid() {
        return Err(Error::new(
            CaptureErrorCode::Internal,
            "failed to create share border overlay brush",
            true,
        ));
    }

    let class_name = wide(CLASS_NAME);
    let class = WNDCLASSW {
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(window_proc),
        hbrBackground: brush,
        lpszClassName: PCWSTR(class_name.as_ptr()),
        ..Default::default()
    };

    let atom = unsafe { RegisterClassW(&class) };
    if atom == 0 {
        let error = unsafe { GetLastError() };
        unsafe {
            let _ = DeleteObject(HGDIOBJ(brush.0));
        }

        if error != ERROR_CLASS_ALREADY_EXISTS {
            return Err(Error::new(
                CaptureErrorCode::Internal,
                format!("failed to register share border overlay window class: {error:?}"),
                true,
            ));
        }
    }

    Ok(())
}

extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
}

fn rgb_to_colorref(color: u32) -> u32 {
    ((color & 0x0000_00ff) << 16) | (color & 0x0000_ff00) | ((color & 0x00ff_0000) >> 16)
}

fn windows_error(context: &str, error: WindowsError) -> Error {
    Error::new(
        CaptureErrorCode::Internal,
        format!("{context}: {error}"),
        true,
    )
}

fn wide(value: &str) -> Vec<u16> {
    OsStr::new(value)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    trait AmbiguousIfSend<A> {
        fn assert_not_send() {}
    }

    impl<T: ?Sized> AmbiguousIfSend<()> for T {}

    struct Invalid;

    impl<T: ?Sized + Send> AmbiguousIfSend<Invalid> for T {}

    #[test]
    fn overlay_windows_are_not_send() {
        <OverlayWindow as AmbiguousIfSend<_>>::assert_not_send();
    }

    #[test]
    fn window_capture_overlay_uses_owned_non_topmost_placement() {
        let placement = OverlayPlacement::for_window(0x2a);

        assert_eq!(placement.owner_hwnd, Some(0x2a));
        assert!(!placement.topmost);
    }

    #[test]
    fn display_capture_overlay_uses_topmost_placement() {
        let placement = OverlayPlacement::for_display();

        assert_eq!(placement.owner_hwnd, None);
        assert!(placement.topmost);
    }

    #[test]
    fn window_visible_bounds_prefer_dwm_frame_bounds() {
        let get_window_rect = OverlayRect {
            left: 96,
            top: 96,
            right: 504,
            bottom: 404,
        };
        let dwm_frame_bounds = OverlayRect {
            left: 100,
            top: 100,
            right: 500,
            bottom: 400,
        };

        assert_eq!(
            preferred_window_bounds(get_window_rect, Some(dwm_frame_bounds)),
            dwm_frame_bounds
        );
    }

    #[test]
    fn win_event_registry_dispatches_only_to_hook_owner_for_same_window() {
        let hwnd = 0x1234usize;
        let first_hook = 0xaaa1usize;
        let second_hook = 0xaaa2usize;
        let first_id = next_window_event_registration_id();
        let second_id = next_window_event_registration_id();
        let (first_sender, first_receiver) = mpsc::channel();
        let (second_sender, second_receiver) = mpsc::channel();

        register_window_event_target(
            first_id,
            hwnd,
            "window:1234".to_string(),
            OverlayStyle::default(),
            first_sender,
        )
        .unwrap();
        register_window_event_hook_handles(first_id, hwnd, &[first_hook]).unwrap();
        register_window_event_target(
            second_id,
            hwnd,
            "window:1234".to_string(),
            OverlayStyle::default(),
            second_sender,
        )
        .unwrap();
        register_window_event_hook_handles(second_id, hwnd, &[second_hook]).unwrap();

        dispatch_window_event(first_hook, EVENT_SYSTEM_MOVESIZESTART, hwnd, OBJID_WINDOW.0);

        assert!(matches!(
            first_receiver.recv_timeout(Duration::from_millis(50)),
            Ok(OverlayCommand::HideForWindowEvent)
        ));
        assert!(matches!(
            second_receiver.recv_timeout(Duration::from_millis(50)),
            Err(mpsc::RecvTimeoutError::Timeout | mpsc::RecvTimeoutError::Disconnected)
        ));

        dispatch_window_event(
            second_hook,
            EVENT_SYSTEM_MOVESIZESTART,
            hwnd,
            OBJID_WINDOW.0,
        );

        assert!(matches!(
            second_receiver.recv_timeout(Duration::from_millis(50)),
            Ok(OverlayCommand::HideForWindowEvent)
        ));

        unregister_window_event_target(first_id);
        unregister_window_event_target(second_id);
    }

    #[test]
    fn win_event_registry_unregisters_one_session_without_removing_others() {
        let hwnd = 0x5678usize;
        let first_hook = 0xbbb1usize;
        let second_hook = 0xbbb2usize;
        let first_id = next_window_event_registration_id();
        let second_id = next_window_event_registration_id();
        let (first_sender, first_receiver) = mpsc::channel();
        let (second_sender, second_receiver) = mpsc::channel();

        register_window_event_target(
            first_id,
            hwnd,
            "window:5678".to_string(),
            OverlayStyle::default(),
            first_sender,
        )
        .unwrap();
        register_window_event_hook_handles(first_id, hwnd, &[first_hook]).unwrap();
        register_window_event_target(
            second_id,
            hwnd,
            "window:5678".to_string(),
            OverlayStyle::default(),
            second_sender,
        )
        .unwrap();
        register_window_event_hook_handles(second_id, hwnd, &[second_hook]).unwrap();
        unregister_window_event_target(first_id);

        dispatch_window_event(first_hook, EVENT_SYSTEM_MOVESIZESTART, hwnd, OBJID_WINDOW.0);

        assert!(matches!(
            first_receiver.recv_timeout(Duration::from_millis(50)),
            Err(mpsc::RecvTimeoutError::Timeout | mpsc::RecvTimeoutError::Disconnected)
        ));

        dispatch_window_event(
            second_hook,
            EVENT_SYSTEM_MOVESIZESTART,
            hwnd,
            OBJID_WINDOW.0,
        );

        assert!(matches!(
            second_receiver.recv_timeout(Duration::from_millis(50)),
            Ok(OverlayCommand::HideForWindowEvent)
        ));

        unregister_window_event_target(second_id);
    }

    #[test]
    fn win_event_registry_destroy_unregisters_target_as_terminal() {
        let hwnd = 0x9abcusize;
        let hook = 0xccc1usize;
        let registration_id = next_window_event_registration_id();
        let (sender, receiver) = mpsc::channel();

        register_window_event_target(
            registration_id,
            hwnd,
            "window:9abc".to_string(),
            OverlayStyle::default(),
            sender,
        )
        .unwrap();
        register_window_event_hook_handles(registration_id, hwnd, &[hook]).unwrap();

        dispatch_window_event(hook, EVENT_OBJECT_DESTROY, hwnd, OBJID_WINDOW.0);

        assert!(matches!(
            receiver.recv_timeout(Duration::from_millis(50)),
            Ok(OverlayCommand::HideForWindowEvent)
        ));
        assert!(matches!(
            receiver.recv_timeout(Duration::from_millis(50)),
            Ok(OverlayCommand::ClearWindowEventTargetForWindowEvent { registration_id: id })
                if id == registration_id
        ));

        dispatch_window_event(hook, EVENT_SYSTEM_MOVESIZESTART, hwnd, OBJID_WINDOW.0);

        assert!(matches!(
            receiver.recv_timeout(Duration::from_millis(50)),
            Err(mpsc::RecvTimeoutError::Timeout | mpsc::RecvTimeoutError::Disconnected)
        ));

        unregister_window_event_target(registration_id);
    }
}
