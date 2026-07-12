use std::ffi::c_void;

use windows::Win32::{
    Foundation::{HWND, RECT},
    Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_EXTENDED_FRAME_BOUNDS},
    Graphics::Gdi::{GetMonitorInfoW, HMONITOR, MONITORINFO},
    UI::WindowsAndMessaging::{GetWindowRect, IsIconic, IsWindow, IsWindowVisible},
};
use windows_capture::monitor::Monitor;

use crate::{error::Error, models::CaptureErrorCode, overlay::OverlayRect, Result};

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

pub(crate) fn display_bounds_from_source_id(source_id: &str) -> Option<OverlayRect> {
    let index = windows_display_index_from_source_id(source_id)?;
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

pub(crate) fn window_bounds_from_source_id(source_id: &str) -> Result<Option<OverlayRect>> {
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

pub(crate) fn preferred_window_bounds(
    get_window_rect: OverlayRect,
    dwm_frame_bounds: Option<OverlayRect>,
) -> OverlayRect {
    dwm_frame_bounds.unwrap_or(get_window_rect)
}
