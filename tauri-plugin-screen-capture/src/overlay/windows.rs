use std::{
    ffi::OsStr,
    marker::PhantomData,
    os::windows::ffi::OsStrExt,
    rc::Rc,
    sync::{mpsc, Mutex},
    thread::{self, JoinHandle},
    time::Duration,
};

use async_trait::async_trait;
use windows::{
    core::{Error as WindowsError, PCWSTR},
    Win32::{
        Foundation::{
            GetLastError, COLORREF, ERROR_CLASS_ALREADY_EXISTS, HWND, LPARAM, LRESULT, WPARAM,
        },
        Graphics::Gdi::{CreateSolidBrush, DeleteObject, HGDIOBJ},
        UI::WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, PeekMessageW,
            RegisterClassW, SetLayeredWindowAttributes, SetWindowDisplayAffinity, SetWindowPos,
            ShowWindow, TranslateMessage, CS_HREDRAW, CS_VREDRAW, HMENU, HWND_TOPMOST, LWA_ALPHA,
            MSG, PM_REMOVE, SWP_NOACTIVATE, SWP_NOOWNERZORDER, SW_HIDE, SW_SHOWNOACTIVATE,
            WDA_EXCLUDEFROMCAPTURE, WM_QUIT, WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE,
            WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
        },
    },
};

use crate::{
    error::Error,
    models::CaptureErrorCode,
    overlay::{corner_segments, OverlayRect, OverlayStyle, OverlayTarget, ShareOverlay},
    Result,
};

const CLASS_NAME: &str = "TauriPluginScreenCaptureShareBorder";
const COMMAND_POLL_INTERVAL: Duration = Duration::from_millis(16);

#[derive(Debug)]
pub struct WindowsShareOverlay {
    style: OverlayStyle,
    host: Mutex<Option<OverlayHost>>,
}

impl Default for WindowsShareOverlay {
    fn default() -> Self {
        Self {
            style: OverlayStyle::default(),
            host: Mutex::new(None),
        }
    }
}

#[async_trait]
impl ShareOverlay for WindowsShareOverlay {
    async fn start(&self, _target: OverlayTarget) -> Result<()> {
        self.show_rect(OverlayRect {
            left: 100,
            top: 100,
            right: 500,
            bottom: 400,
        })
        .await
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
                window.position(segment.x, segment.y, segment.width, segment.height);
                window.show();
                windows.push(window);
            }

            self.windows = windows;
            return Ok(());
        }

        for (window, segment) in self.windows.iter().zip(segments) {
            window.position(segment.x, segment.y, segment.width, segment.height);
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

    fn position(&self, x: i32, y: i32, width: i32, height: i32) {
        unsafe {
            let _ = SetWindowPos(
                self.hwnd,
                Some(HWND_TOPMOST),
                x,
                y,
                width,
                height,
                SWP_NOACTIVATE | SWP_NOOWNERZORDER,
            );
        }
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
}
