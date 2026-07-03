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
        Graphics::Gdi::{CreateSolidBrush, DeleteObject, HGDIOBJ},
        UI::Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK},
        UI::WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetWindowRect,
            GetWindowThreadProcessId, IsIconic, IsWindow, IsWindowVisible, PeekMessageW,
            RegisterClassW, SetLayeredWindowAttributes, SetWindowDisplayAffinity, SetWindowPos,
            ShowWindow, TranslateMessage, CS_HREDRAW, CS_VREDRAW, EVENT_OBJECT_DESTROY,
            EVENT_OBJECT_HIDE, EVENT_OBJECT_LOCATIONCHANGE, EVENT_OBJECT_SHOW,
            EVENT_SYSTEM_MOVESIZEEND, EVENT_SYSTEM_MOVESIZESTART, HMENU, HWND_TOPMOST, LWA_ALPHA,
            MSG, OBJID_WINDOW, PM_REMOVE, SWP_NOACTIVATE, SWP_NOOWNERZORDER, SW_HIDE,
            SW_SHOWNOACTIVATE, WDA_EXCLUDEFROMCAPTURE, WINEVENT_OUTOFCONTEXT, WM_QUIT, WNDCLASSW,
            WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT,
            WS_POPUP,
        },
    },
};

use crate::{
    error::Error,
    models::{CaptureErrorCode, CaptureSourceKind},
    overlay::{corner_segments, OverlayRect, OverlayStyle, OverlayTarget, ShareOverlay},
    Result,
};

const CLASS_NAME: &str = "TauriPluginScreenCaptureShareBorder";
const COMMAND_POLL_INTERVAL: Duration = Duration::from_millis(16);
static NEXT_WINDOW_EVENT_REGISTRATION_ID: AtomicU64 = AtomicU64::new(1);
static WINDOW_EVENT_REGISTRY: OnceLock<Mutex<HashMap<usize, Vec<WindowEventTarget>>>> =
    OnceLock::new();

#[derive(Debug)]
pub struct WindowsShareOverlay {
    style: OverlayStyle,
    host: Mutex<Option<OverlayHost>>,
    window_event_hooks: Mutex<Option<WindowEventHooks>>,
    window_event_registration_id: u64,
}

impl Default for WindowsShareOverlay {
    fn default() -> Self {
        Self {
            style: OverlayStyle::default(),
            host: Mutex::new(None),
            window_event_hooks: Mutex::new(None),
            window_event_registration_id: next_window_event_registration_id(),
        }
    }
}

#[async_trait]
impl ShareOverlay for WindowsShareOverlay {
    async fn start(&self, target: OverlayTarget) -> Result<()> {
        match target.source_kind {
            CaptureSourceKind::Window => {
                let target_hwnd = windows_target_handle_from_source_id(&target.source_id)
                    .ok_or_else(|| {
                        Error::new(
                            CaptureErrorCode::SourceNotFound,
                            format!("invalid Windows window source id: {}", target.source_id),
                            false,
                        )
                    })?;
                let rect = window_bounds_from_source_id(&target.source_id)?;
                let sender = self.ensure_host_sender()?;
                self.replace_window_event_hooks(
                    target_hwnd,
                    target.source_id.clone(),
                    sender.clone(),
                )?;

                match rect {
                    Some(rect) => {
                        let style = self.style;
                        send_overlay_command(sender, move |reply| OverlayCommand::ShowRect {
                            rect,
                            style,
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
                self.clear_window_event_hooks()?;
                self.show_rect(OverlayRect {
                    left: 100,
                    top: 100,
                    right: 500,
                    bottom: 400,
                })
                .await
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
        self.clear_window_event_hooks()?;

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

        send_overlay_command(sender, move |reply| OverlayCommand::ShowRect {
            rect,
            style,
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

    fn replace_window_event_hooks(
        &self,
        target_hwnd: usize,
        source_id: String,
        sender: mpsc::Sender<OverlayCommand>,
    ) -> Result<()> {
        self.clear_window_event_hooks()?;

        let process_id = window_process_id(HWND(target_hwnd as *mut c_void));
        let handles = install_window_event_hooks(process_id)?;
        let mut hook_state = match self.lock_window_event_hooks() {
            Ok(hook_state) => hook_state,
            Err(error) => {
                for handle in handles {
                    unsafe {
                        let _ = UnhookWinEvent(HWINEVENTHOOK(handle as *mut c_void));
                    }
                }
                return Err(error);
            }
        };
        register_window_event_target(
            self.window_event_registration_id,
            target_hwnd,
            source_id,
            self.style,
            sender,
        );

        *hook_state = Some(WindowEventHooks {
            target_hwnd,
            registration_id: self.window_event_registration_id,
            handles,
        });

        Ok(())
    }

    fn clear_window_event_hooks(&self) -> Result<()> {
        if let Some(hooks) = self.lock_window_event_hooks()?.take() {
            hooks.unregister();
        }

        Ok(())
    }

    fn lock_window_event_hooks(
        &self,
    ) -> Result<std::sync::MutexGuard<'_, Option<WindowEventHooks>>> {
        self.window_event_hooks.lock().map_err(|_| {
            Error::new(
                CaptureErrorCode::Internal,
                "share border overlay WinEvent hook state lock was poisoned",
                false,
            )
        })
    }
}

impl Drop for WindowsShareOverlay {
    fn drop(&mut self) {
        if let Ok(mut hooks) = self.window_event_hooks.lock() {
            if let Some(hooks) = hooks.take() {
                hooks.unregister();
            }
        }
    }
}

pub fn windows_target_handle_from_source_id(source_id: &str) -> Option<usize> {
    source_id
        .strip_prefix("window:")
        .and_then(|id| usize::from_str_radix(id, 16).ok())
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

    let mut rect = RECT::default();
    unsafe { GetWindowRect(hwnd, &mut rect) }.map_err(|error| {
        Error::new(
            CaptureErrorCode::SourceUnavailable,
            format!("failed to resolve Windows window bounds for {source_id}: {error}"),
            true,
        )
    })?;

    Ok(Some(OverlayRect {
        left: rect.left,
        top: rect.top,
        right: rect.right,
        bottom: rect.bottom,
    }))
}

#[derive(Debug)]
struct WindowEventHooks {
    target_hwnd: usize,
    registration_id: u64,
    handles: Vec<usize>,
}

impl WindowEventHooks {
    fn unregister(self) {
        unregister_window_event_target(self.registration_id, self.target_hwnd);

        for handle in self.handles {
            unsafe {
                let _ = UnhookWinEvent(HWINEVENTHOOK(handle as *mut c_void));
            }
        }
    }
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
    RefreshBounds {
        source_id: String,
        style: OverlayStyle,
        sender: mpsc::Sender<OverlayCommand>,
    },
}

fn next_window_event_registration_id() -> u64 {
    NEXT_WINDOW_EVENT_REGISTRATION_ID.fetch_add(1, Ordering::Relaxed)
}

fn window_event_registry() -> &'static Mutex<HashMap<usize, Vec<WindowEventTarget>>> {
    WINDOW_EVENT_REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

fn register_window_event_target(
    registration_id: u64,
    target_hwnd: usize,
    source_id: String,
    style: OverlayStyle,
    sender: mpsc::Sender<OverlayCommand>,
) {
    let Ok(mut registry) = window_event_registry().lock() else {
        return;
    };
    let targets = registry.entry(target_hwnd).or_default();
    targets.retain(|target| target.registration_id != registration_id);
    targets.push(WindowEventTarget {
        registration_id,
        source_id,
        style,
        sender,
        moving_or_sizing: false,
    });
}

fn unregister_window_event_target(registration_id: u64, target_hwnd: usize) {
    let Ok(mut registry) = window_event_registry().lock() else {
        return;
    };

    if let Some(targets) = registry.get_mut(&target_hwnd) {
        targets.retain(|target| target.registration_id != registration_id);
        if targets.is_empty() {
            registry.remove(&target_hwnd);
        }
    }
}

fn install_window_event_hooks(process_id: u32) -> Result<Vec<usize>> {
    let mut handles = Vec::with_capacity(2);

    for (event_min, event_max) in [
        (EVENT_SYSTEM_MOVESIZESTART, EVENT_SYSTEM_MOVESIZEEND),
        (EVENT_OBJECT_DESTROY, EVENT_OBJECT_LOCATIONCHANGE),
    ] {
        match install_window_event_hook(event_min, event_max, process_id) {
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

fn install_window_event_hook(event_min: u32, event_max: u32, process_id: u32) -> Result<usize> {
    let hook = unsafe {
        SetWinEventHook(
            event_min,
            event_max,
            None,
            Some(win_event_proc),
            process_id,
            0,
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

fn window_process_id(hwnd: HWND) -> u32 {
    let mut process_id = 0;
    unsafe {
        let _ = GetWindowThreadProcessId(hwnd, Some(&mut process_id));
    }
    process_id
}

unsafe extern "system" fn win_event_proc(
    _hook: HWINEVENTHOOK,
    event: u32,
    hwnd: HWND,
    object_id: i32,
    _child_id: i32,
    _event_thread: u32,
    _event_time: u32,
) {
    dispatch_window_event(event, hwnd.0 as usize, object_id);
}

fn dispatch_window_event(event: u32, hwnd: usize, object_id: i32) {
    if object_id != OBJID_WINDOW.0 {
        return;
    }

    let actions = {
        let Ok(mut registry) = window_event_registry().lock() else {
            return;
        };
        let Some(targets) = registry.get_mut(&hwnd) else {
            return;
        };

        let mut actions = Vec::new();
        for target in targets {
            match event {
                EVENT_SYSTEM_MOVESIZESTART => {
                    target.moving_or_sizing = true;
                    actions.push(WindowEventAction::Hide {
                        sender: target.sender.clone(),
                    });
                }
                EVENT_SYSTEM_MOVESIZEEND => {
                    target.moving_or_sizing = false;
                    actions.push(WindowEventAction::RefreshBounds {
                        source_id: target.source_id.clone(),
                        style: target.style,
                        sender: target.sender.clone(),
                    });
                }
                EVENT_OBJECT_HIDE | EVENT_OBJECT_DESTROY => {
                    target.moving_or_sizing = false;
                    actions.push(WindowEventAction::Hide {
                        sender: target.sender.clone(),
                    });
                }
                EVENT_OBJECT_SHOW | EVENT_OBJECT_LOCATIONCHANGE => {
                    if !target.moving_or_sizing {
                        actions.push(WindowEventAction::RefreshBounds {
                            source_id: target.source_id.clone(),
                            style: target.style,
                            sender: target.sender.clone(),
                        });
                    }
                }
                _ => {}
            }
        }
        actions
    };

    for action in actions {
        match action {
            WindowEventAction::Hide { sender } => {
                let _ = sender.send(OverlayCommand::HideForWindowEvent);
            }
            WindowEventAction::RefreshBounds {
                source_id,
                style,
                sender,
            } => {
                let command = match window_bounds_from_source_id(&source_id) {
                    Ok(Some(rect)) => OverlayCommand::ShowRectForWindowEvent { rect, style },
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
    },
    HideForWindowEvent,
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

        self.windows.clear();
    }

    fn handle_command(&mut self, command: OverlayCommand) -> bool {
        match command {
            OverlayCommand::ShowRect { rect, style, reply } => {
                send_reply(reply, self.show_rect(rect, style));
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
            OverlayCommand::ShowRectForWindowEvent { rect, style } => {
                if let Err(error) = self.show_rect(rect, style) {
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
            OverlayCommand::Shutdown { reply } => {
                self.windows.clear();
                send_reply(reply, Ok(()));
                true
            }
        }
    }

    fn show_rect(&mut self, rect: OverlayRect, style: OverlayStyle) -> Result<()> {
        let segments = corner_segments(rect, style);

        if self.windows.len() != segments.len() {
            let mut windows = Vec::with_capacity(segments.len());

            for segment in &segments {
                let window = OverlayWindow::new(style.color)?;
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
    _ui_thread: PhantomData<Rc<()>>,
}

impl OverlayWindow {
    fn new(color: u32) -> Result<Self> {
        register_window_class(color)?;

        let class_name = wide(CLASS_NAME);
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_TOOLWINDOW
                    | WS_EX_TOPMOST
                    | WS_EX_TRANSPARENT
                    | WS_EX_LAYERED
                    | WS_EX_NOACTIVATE,
                PCWSTR(class_name.as_ptr()),
                PCWSTR(class_name.as_ptr()),
                WS_POPUP,
                0,
                0,
                1,
                1,
                None,
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
        unsafe {
            SetWindowPos(
                self.hwnd,
                Some(HWND_TOPMOST),
                x,
                y,
                width,
                height,
                SWP_NOACTIVATE | SWP_NOOWNERZORDER,
            )
            .map_err(|error| {
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
    fn win_event_registry_dispatches_to_multiple_sessions_for_same_window() {
        let hwnd = 0x1234usize;
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
        );
        register_window_event_target(
            second_id,
            hwnd,
            "window:1234".to_string(),
            OverlayStyle::default(),
            second_sender,
        );

        dispatch_window_event(EVENT_SYSTEM_MOVESIZESTART, hwnd, OBJID_WINDOW.0);

        assert!(matches!(
            first_receiver.recv_timeout(Duration::from_millis(50)),
            Ok(OverlayCommand::HideForWindowEvent)
        ));
        assert!(matches!(
            second_receiver.recv_timeout(Duration::from_millis(50)),
            Ok(OverlayCommand::HideForWindowEvent)
        ));

        unregister_window_event_target(first_id, hwnd);
        unregister_window_event_target(second_id, hwnd);
    }

    #[test]
    fn win_event_registry_unregisters_one_session_without_removing_others() {
        let hwnd = 0x5678usize;
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
        );
        register_window_event_target(
            second_id,
            hwnd,
            "window:5678".to_string(),
            OverlayStyle::default(),
            second_sender,
        );
        unregister_window_event_target(first_id, hwnd);

        dispatch_window_event(EVENT_SYSTEM_MOVESIZESTART, hwnd, OBJID_WINDOW.0);

        assert!(matches!(
            first_receiver.recv_timeout(Duration::from_millis(50)),
            Err(mpsc::RecvTimeoutError::Timeout | mpsc::RecvTimeoutError::Disconnected)
        ));
        assert!(matches!(
            second_receiver.recv_timeout(Duration::from_millis(50)),
            Ok(OverlayCommand::HideForWindowEvent)
        ));

        unregister_window_event_target(second_id, hwnd);
    }
}
