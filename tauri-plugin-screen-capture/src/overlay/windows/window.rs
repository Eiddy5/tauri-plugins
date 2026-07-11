use std::{ffi::c_void, marker::PhantomData, rc::Rc};

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
            SWP_NOOWNERZORDER, SWP_NOZORDER, SW_HIDE, SW_SHOWNOACTIVATE, WDA_EXCLUDEFROMCAPTURE,
            WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
            WS_EX_TRANSPARENT, WS_POPUP,
        },
    },
};

use crate::{error::Error, models::CaptureErrorCode, Result};

use super::{
    placement::OverlayPlacement,
    util::{rgb_to_colorref, wide, windows_error},
};

const CLASS_NAME: &str = "TauriPluginScreenCaptureShareBorder";

#[derive(Debug)]
pub(crate) struct OverlayWindow {
    hwnd: HWND,
    pub(crate) placement: OverlayPlacement,
    _ui_thread: PhantomData<Rc<()>>,
}

impl OverlayWindow {
    #[cfg(test)]
    pub(crate) const fn hwnd(&self) -> HWND {
        self.hwnd
    }

    pub(crate) fn new(color: u32, placement: OverlayPlacement) -> Result<Self> {
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

    pub(crate) fn position(&self, x: i32, y: i32, width: i32, height: i32) -> Result<()> {
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

    pub(crate) fn show(&self) {
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
        }
    }

    pub(crate) fn hide(&self) {
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
