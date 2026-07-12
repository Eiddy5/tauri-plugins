use std::{
    collections::HashMap,
    ffi::c_void,
    sync::{mpsc, Arc},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use windows::Win32::{
    Foundation::{CloseHandle, HANDLE, HWND, WAIT_FAILED},
    System::Threading::{CreateEventW, SetEvent},
    UI::{
        Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON},
        WindowsAndMessaging::{
            DispatchMessageW, MsgWaitForMultipleObjectsEx, PeekMessageW, TranslateMessage, MSG,
            MWMO_INPUTAVAILABLE, PM_REMOVE, QS_ALLINPUT, WM_QUIT,
        },
    },
};

use crate::{
    error::Error,
    models::CaptureErrorCode,
    overlay::{corner_segments, OverlayRect, OverlayStyle},
    Result,
};

use super::{
    bounds::window_bounds_from_source_id,
    events::{
        install_window_event_hooks, register_window_event_hook_handles,
        register_window_event_target, target_window_thread, unregister_window_event_target,
        WindowEventHooks,
    },
    placement::{OverlayFocus, OverlayPlacement},
    window::OverlayWindow,
};

const WINDOW_LOCATION_CHANGE_REFRESH_DELAY: Duration = Duration::from_millis(160);

#[derive(Debug)]
struct CommandWake {
    handle: usize,
}

impl CommandWake {
    fn new() -> Result<Self> {
        let handle = unsafe { CreateEventW(None, false, false, None) }.map_err(|error| {
            Error::new(
                CaptureErrorCode::Internal,
                format!("failed to create share border overlay wake event: {error}"),
                true,
            )
        })?;
        Ok(Self {
            handle: handle.0 as usize,
        })
    }

    fn handle(&self) -> HANDLE {
        HANDLE(self.handle as *mut c_void)
    }

    fn wake(&self) -> std::result::Result<(), ()> {
        unsafe { SetEvent(self.handle()) }.map_err(|_| ())
    }
}

impl Drop for CommandWake {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.handle());
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct OverlayCommandSender {
    commands: mpsc::Sender<OverlayCommand>,
    wake: Option<Arc<CommandWake>>,
}

impl OverlayCommandSender {
    #[cfg(test)]
    pub(crate) fn without_wake(commands: mpsc::Sender<OverlayCommand>) -> Self {
        Self {
            commands,
            wake: None,
        }
    }

    pub(crate) fn send(&self, command: OverlayCommand) -> std::result::Result<(), ()> {
        self.commands.send(command).map_err(|_| ())?;
        if let Some(wake) = &self.wake {
            wake.wake()?;
        }
        Ok(())
    }
}

#[derive(Debug)]
pub(crate) struct OverlayHost {
    pub(crate) sender: OverlayCommandSender,
    thread: Option<JoinHandle<()>>,
}

impl OverlayHost {
    pub(crate) fn spawn() -> Result<Self> {
        let (commands, receiver) = mpsc::channel();
        let wake = Arc::new(CommandWake::new()?);
        let thread_wake = Arc::clone(&wake);
        let thread = thread::Builder::new()
            .name("tauri-screen-capture-share-overlay".into())
            .spawn(move || OverlayThread::default().run(receiver, thread_wake))
            .map_err(|error| {
                Error::new(
                    CaptureErrorCode::Internal,
                    format!("failed to start share border overlay UI thread: {error}"),
                    true,
                )
            })?;

        Ok(Self {
            sender: OverlayCommandSender {
                commands,
                wake: Some(wake),
            },
            thread: Some(thread),
        })
    }

    pub(crate) async fn shutdown(self) -> Result<()> {
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

pub(crate) enum OverlayCommand {
    ShowRect {
        rect: OverlayRect,
        style: OverlayStyle,
        placement: OverlayPlacement,
        focus: OverlayFocus,
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
    DeferWindowEventRefresh {
        registration_id: u64,
        source_id: String,
        style: OverlayStyle,
        target_hwnd: usize,
    },
    SuspendWindowLocationChangeHook {
        registration_id: u64,
        clear_deferred_refresh: bool,
    },
    ResumeWindowLocationChangeHook {
        registration_id: u64,
    },
    RegisterWindowEventTarget {
        registration_id: u64,
        target_hwnd: usize,
        source_id: String,
        style: OverlayStyle,
        event_sender: OverlayCommandSender,
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
    pub(crate) fn show(reply: OverlayReply) -> Self {
        Self::Show { reply }
    }

    pub(crate) fn hide(reply: OverlayReply) -> Self {
        Self::Hide { reply }
    }

    pub(crate) fn shutdown(reply: OverlayReply) -> Self {
        Self::Shutdown { reply }
    }
}

pub(crate) async fn send_overlay_command(
    sender: OverlayCommandSender,
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
pub(crate) struct OverlayThread {
    windows: Vec<OverlayWindow>,
    window_event_hooks: HashMap<u64, WindowEventHooks>,
    pub(crate) pending_window_event_refreshes: HashMap<u64, PendingWindowEventRefresh>,
}

pub(crate) struct PendingWindowEventRefresh {
    pub(crate) source_id: String,
    pub(crate) style: OverlayStyle,
    pub(crate) target_hwnd: usize,
    pub(crate) due_at: Instant,
}

impl OverlayThread {
    fn run(mut self, receiver: mpsc::Receiver<OverlayCommand>, wake: Arc<CommandWake>) {
        loop {
            if !pump_window_messages() {
                break;
            }
            if self.handle_queued_commands(&receiver) {
                break;
            }
            self.refresh_due_window_event_targets();
            let timeout_ms = self.next_refresh_timeout_ms();
            let handles = [wake.handle()];
            let wait_result = unsafe {
                MsgWaitForMultipleObjectsEx(
                    Some(&handles),
                    timeout_ms,
                    QS_ALLINPUT,
                    MWMO_INPUTAVAILABLE,
                )
            };
            if wait_result == WAIT_FAILED {
                eprintln!("[screen-capture] Windows overlay wait failed; stopping overlay thread");
                break;
            }
        }

        self.clear_all_window_event_targets();
        self.windows.clear();
    }

    fn next_refresh_timeout_ms(&self) -> u32 {
        let Some(due_at) = self
            .pending_window_event_refreshes
            .values()
            .map(|refresh| refresh.due_at)
            .min()
        else {
            return u32::MAX;
        };
        let remaining = due_at.saturating_duration_since(Instant::now());
        u32::try_from(remaining.as_millis().max(1)).unwrap_or(u32::MAX - 1)
    }

    pub(crate) fn handle_queued_commands(
        &mut self,
        receiver: &mpsc::Receiver<OverlayCommand>,
    ) -> bool {
        loop {
            match receiver.try_recv() {
                Ok(command) => {
                    if self.handle_command(command) {
                        return true;
                    }
                }
                Err(mpsc::TryRecvError::Empty) => return false,
                Err(mpsc::TryRecvError::Disconnected) => return true,
            }
        }
    }

    fn handle_command(&mut self, command: OverlayCommand) -> bool {
        match command {
            OverlayCommand::ShowRect {
                rect,
                style,
                placement,
                focus,
                reply,
            } => {
                send_reply(reply, self.show_rect(rect, style, placement, focus));
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
                if let Err(error) = self.show_rect(rect, style, placement, OverlayFocus::none()) {
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
            OverlayCommand::DeferWindowEventRefresh {
                registration_id,
                source_id,
                style,
                target_hwnd,
            } => {
                self.defer_window_event_refresh(registration_id, source_id, style, target_hwnd);
                false
            }
            OverlayCommand::SuspendWindowLocationChangeHook {
                registration_id,
                clear_deferred_refresh,
            } => {
                self.suspend_window_location_change_hook(registration_id, clear_deferred_refresh);
                false
            }
            OverlayCommand::ResumeWindowLocationChangeHook { registration_id } => {
                if let Err(error) = self.resume_window_location_change_hook(registration_id) {
                    eprintln!(
                        "[screen-capture] failed to resume share overlay location-change hook: {error}"
                    );
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
                self.pending_window_event_refreshes.clear();
                send_reply(reply, Ok(()));
                true
            }
        }
    }

    pub(crate) fn defer_window_event_refresh(
        &mut self,
        registration_id: u64,
        source_id: String,
        style: OverlayStyle,
        target_hwnd: usize,
    ) {
        for window in &self.windows {
            window.hide();
        }

        self.pending_window_event_refreshes.insert(
            registration_id,
            PendingWindowEventRefresh {
                source_id,
                style,
                target_hwnd,
                due_at: Instant::now() + WINDOW_LOCATION_CHANGE_REFRESH_DELAY,
            },
        );
    }

    pub(crate) fn suspend_window_location_change_hook(
        &mut self,
        registration_id: u64,
        clear_deferred_refresh: bool,
    ) {
        if clear_deferred_refresh {
            self.pending_window_event_refreshes.remove(&registration_id);
        }
        if let Some(hooks) = self.window_event_hooks.get_mut(&registration_id) {
            hooks.suspend_location_change();
        }
    }

    fn resume_window_location_change_hook(&mut self, registration_id: u64) -> Result<()> {
        let Some(hooks) = self.window_event_hooks.get_mut(&registration_id) else {
            return Ok(());
        };

        hooks.resume_location_change()
    }

    pub(crate) fn refresh_due_window_event_targets(&mut self) {
        let now = Instant::now();
        let pointer_button_down = pointer_button_is_down();
        let due_registration_ids = self
            .pending_window_event_refreshes
            .iter()
            .filter_map(|(registration_id, refresh)| {
                deferred_refresh_is_ready(now, refresh.due_at, pointer_button_down)
                    .then_some(*registration_id)
            })
            .collect::<Vec<_>>();

        if pointer_button_down {
            for refresh in self.pending_window_event_refreshes.values_mut() {
                if refresh.due_at <= now {
                    refresh.due_at = now + WINDOW_LOCATION_CHANGE_REFRESH_DELAY;
                }
            }
        }

        for registration_id in due_registration_ids {
            let Some(refresh) = self.pending_window_event_refreshes.remove(&registration_id) else {
                continue;
            };

            let command = match window_bounds_from_source_id(&refresh.source_id) {
                Ok(Some(rect)) => Some((
                    rect,
                    refresh.style,
                    OverlayPlacement::for_window(refresh.target_hwnd),
                )),
                Ok(None) | Err(_) => None,
            };

            if let Some((rect, style, placement)) = command {
                if let Err(error) = self.show_rect(rect, style, placement, OverlayFocus::none()) {
                    eprintln!(
                        "[screen-capture] failed to refresh delayed share overlay from WinEvent: {error}"
                    );
                }
            } else {
                for window in &self.windows {
                    window.hide();
                }
            }
            if let Err(error) = self.resume_window_location_change_hook(registration_id) {
                eprintln!(
                    "[screen-capture] failed to resume delayed share overlay location-change hook: {error}"
                );
            }
        }
    }

    fn show_rect(
        &mut self,
        rect: OverlayRect,
        style: OverlayStyle,
        placement: OverlayPlacement,
        focus: OverlayFocus,
    ) -> Result<()> {
        let segments = corner_segments(rect, style);
        focus.apply();

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
        event_sender: OverlayCommandSender,
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

        let installed_hooks = install_window_event_hooks(process_id, thread_id)?;
        let hooks = WindowEventHooks {
            registration_id,
            target_hwnd,
            process_id,
            thread_id,
            handles: installed_hooks.handles,
            location_change_handle: installed_hooks.location_change_handle,
        };

        if let Err(error) = register_window_event_target(
            registration_id,
            target_hwnd,
            source_id,
            style,
            event_sender,
        )
        .and_then(|_| {
            register_window_event_hook_handles(registration_id, target_hwnd, &hooks.all_handles())
        }) {
            hooks.unregister();
            return Err(error);
        }

        self.window_event_hooks.insert(registration_id, hooks);
        Ok(())
    }

    pub(crate) fn clear_window_event_target(&mut self, registration_id: u64) {
        self.pending_window_event_refreshes.remove(&registration_id);
        if let Some(hooks) = self.window_event_hooks.remove(&registration_id) {
            hooks.unregister();
        } else {
            unregister_window_event_target(registration_id);
        }
    }

    fn clear_all_window_event_targets(&mut self) {
        self.pending_window_event_refreshes.clear();
        let hooks = std::mem::take(&mut self.window_event_hooks);
        for (_, hooks) in hooks {
            hooks.unregister();
        }
    }
}

pub(crate) fn deferred_refresh_is_ready(
    now: Instant,
    due_at: Instant,
    pointer_button_down: bool,
) -> bool {
    !pointer_button_down && due_at <= now
}

fn pointer_button_is_down() -> bool {
    const KEY_DOWN_MASK: i16 = i16::MIN;
    unsafe { GetAsyncKeyState(VK_LBUTTON.0.into()) & KEY_DOWN_MASK != 0 }
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
