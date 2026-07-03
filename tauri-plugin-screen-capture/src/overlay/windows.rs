use std::{
    ffi::OsStr,
    os::windows::ffi::OsStrExt,
    sync::{Mutex, MutexGuard},
};

use async_trait::async_trait;
use windows::{
    core::PCWSTR,
    Win32::{
        Foundation::{
            GetLastError, COLORREF, ERROR_CLASS_ALREADY_EXISTS, HWND, LPARAM, LRESULT, WPARAM,
        },
        Graphics::Gdi::{CreateSolidBrush, DeleteObject, HGDIOBJ},
        UI::WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, DestroyWindow, RegisterClassW,
            SetLayeredWindowAttributes, SetWindowDisplayAffinity, SetWindowPos, ShowWindow,
            CS_HREDRAW, CS_VREDRAW, HMENU, HWND_TOPMOST, LWA_ALPHA, SWP_NOACTIVATE,
            SWP_NOOWNERZORDER, SW_HIDE, SW_SHOWNOACTIVATE, WDA_EXCLUDEFROMCAPTURE, WNDCLASSW,
            WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT,
            WS_POPUP,
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

#[derive(Debug)]
pub struct WindowsShareOverlay {
    style: OverlayStyle,
    windows: Mutex<Vec<OverlayWindow>>,
}

impl Default for WindowsShareOverlay {
    fn default() -> Self {
        Self {
            style: OverlayStyle::default(),
            windows: Mutex::new(Vec::new()),
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
    }

    async fn show(&self) -> Result<()> {
        for window in self.lock_windows()?.iter() {
            window.show();
        }

        Ok(())
    }

    async fn hide(&self) -> Result<()> {
        for window in self.lock_windows()?.iter() {
            window.hide();
        }

        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        self.destroy_all()
    }
}

impl WindowsShareOverlay {
    fn show_rect(&self, rect: OverlayRect) -> Result<()> {
        let segments = corner_segments(rect, self.style);
        let mut windows = self.lock_windows()?;

        if windows.len() != segments.len() {
            windows.clear();
            windows.reserve(segments.len());

            for segment in &segments {
                let window = OverlayWindow::new(self.style.color)?;
                window.position(segment.x, segment.y, segment.width, segment.height);
                window.show();
                windows.push(window);
            }

            return Ok(());
        }

        for (window, segment) in windows.iter().zip(segments) {
            window.position(segment.x, segment.y, segment.width, segment.height);
            window.show();
        }

        Ok(())
    }

    fn destroy_all(&self) -> Result<()> {
        self.lock_windows()?.clear();
        Ok(())
    }

    fn lock_windows(&self) -> Result<MutexGuard<'_, Vec<OverlayWindow>>> {
        self.windows.lock().map_err(|_| {
            Error::new(
                CaptureErrorCode::Internal,
                "share border overlay window state lock was poisoned",
                false,
            )
        })
    }
}

#[derive(Debug)]
struct OverlayWindow {
    hwnd: HWND,
}

unsafe impl Send for OverlayWindow {}

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

        unsafe {
            let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), 255, LWA_ALPHA);
            let _ = SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE);
        }

        Ok(Self { hwnd })
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

fn wide(value: &str) -> Vec<u16> {
    OsStr::new(value)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}
