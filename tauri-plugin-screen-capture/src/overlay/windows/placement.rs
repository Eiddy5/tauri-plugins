use std::ffi::c_void;

use windows::Win32::{Foundation::HWND, UI::WindowsAndMessaging::SetForegroundWindow};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct OverlayPlacement {
    pub(crate) owner_hwnd: Option<usize>,
    pub(crate) topmost: bool,
}

impl OverlayPlacement {
    pub(crate) const fn for_window(owner_hwnd: usize) -> Self {
        Self {
            owner_hwnd: Some(owner_hwnd),
            topmost: false,
        }
    }

    pub(crate) const fn for_display() -> Self {
        Self {
            owner_hwnd: None,
            topmost: true,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum OverlayFocus {
    None,
    OwnerOnce(usize),
}

impl OverlayFocus {
    pub(crate) const fn for_window_start(owner_hwnd: usize) -> Self {
        Self::OwnerOnce(owner_hwnd)
    }

    pub(crate) const fn none() -> Self {
        Self::None
    }

    pub(crate) fn apply(self) {
        if let Self::OwnerOnce(owner_hwnd) = self {
            unsafe {
                let _ = SetForegroundWindow(HWND(owner_hwnd as *mut c_void));
            }
        }
    }
}
