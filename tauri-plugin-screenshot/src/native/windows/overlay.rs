use std::{ffi::c_void, mem::size_of};

use async_trait::async_trait;
use windows::{
    core::{BOOL, PCWSTR},
    Win32::{
        Foundation::{
            GetLastError, ERROR_CLASS_ALREADY_EXISTS, HINSTANCE, HWND, LPARAM, LRESULT, POINT,
            RECT, WPARAM,
        },
        Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS},
        Graphics::Gdi::{
            GetMonitorInfoW, InvalidateRect, MonitorFromPoint, ValidateRect, MONITORINFO,
            MONITOR_DEFAULTTONEAREST,
        },
        System::LibraryLoader::GetModuleHandleW,
        UI::{
            HiDpi::{
                GetDpiForMonitor, SetThreadDpiAwarenessContext,
                DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE,
                DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, MDT_EFFECTIVE_DPI,
            },
            Input::KeyboardAndMouse::{ReleaseCapture, SetCapture, SetFocus, VK_ESCAPE, VK_RETURN},
            WindowsAndMessaging::{
                CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, EnumWindows,
                GetCursorPos, GetMessageW, GetSystemMetrics, GetWindowLongPtrW, GetWindowLongW,
                GetWindowRect, IsWindowVisible, LoadCursorW, PostQuitMessage, RegisterClassW,
                SetCursor, SetForegroundWindow, SetWindowLongPtrW, SetWindowPos, ShowWindow,
                TranslateMessage, CREATESTRUCTW, CS_DBLCLKS, CS_HREDRAW, CS_VREDRAW, GWLP_USERDATA,
                GWL_EXSTYLE, HMENU, HWND_TOPMOST, IDC_ARROW, IDC_CROSS, MSG, SM_CXVIRTUALSCREEN,
                SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, SWP_NOOWNERZORDER,
                SW_SHOW, WM_CLOSE, WM_DESTROY, WM_DISPLAYCHANGE, WM_ERASEBKGND, WM_KEYDOWN,
                WM_LBUTTONDBLCLK, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_NCCREATE,
                WM_PAINT, WM_RBUTTONDOWN, WM_SETCURSOR, WNDCLASSW, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
                WS_POPUP,
            },
        },
    },
};

use crate::{
    error::{Error, ErrorCode},
    models::CaptureOptions,
    selection::{Interaction, Point, Rect, SelectionModel, Size},
    Result,
};

use super::super::{
    desktop::{DesktopRect, WindowCatalog},
    magnifier::layout_magnifier,
    NativeCaptureAdapter, NativeCaptureOutcome,
};
use super::{
    d2d::{Direct2DOverlay, OverlayScene},
    frame::{CapturedFrame, PixelFormat},
    wgc,
};

const CLASS_NAME: &str = "TauriPluginNativeScreenshotOverlay";

#[derive(Debug, Default)]
pub(crate) struct WindowsNativeCaptureAdapter;

#[async_trait]
impl NativeCaptureAdapter for WindowsNativeCaptureAdapter {
    async fn capture_area(&self, options: CaptureOptions) -> Result<NativeCaptureOutcome> {
        tokio::task::spawn_blocking(move || run_native_capture(options))
            .await
            .map_err(|error| {
                Error::new(
                    ErrorCode::CaptureFailed,
                    format!("Windows 截图线程异常结束：{error}"),
                    true,
                )
            })?
    }
}

fn run_native_capture(options: CaptureOptions) -> Result<NativeCaptureOutcome> {
    let mut previous_dpi =
        unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    if previous_dpi.is_invalid() {
        previous_dpi =
            unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE) };
    }
    let result = run_native_capture_inner(options);
    if !previous_dpi.is_invalid() {
        unsafe {
            SetThreadDpiAwarenessContext(previous_dpi);
        }
    }
    result
}

fn run_native_capture_inner(options: CaptureOptions) -> Result<NativeCaptureOutcome> {
    let desktop_rect = virtual_desktop_rect()?;
    let desktop = desktop_rect_from_win32(desktop_rect);
    let ui_scale = ui_scale_under_cursor()?;
    let width =
        u32::try_from(desktop_rect.right.saturating_sub(desktop_rect.left)).map_err(|_| {
            Error::new(
                ErrorCode::DisplayUnavailable,
                "Windows 虚拟桌面宽度无效",
                true,
            )
        })?;
    let height =
        u32::try_from(desktop_rect.bottom.saturating_sub(desktop_rect.top)).map_err(|_| {
            Error::new(
                ErrorCode::DisplayUnavailable,
                "Windows 虚拟桌面高度无效",
                true,
            )
        })?;
    if width == 0 || height == 0 {
        return Err(Error::new(
            ErrorCode::DisplayUnavailable,
            "Windows 虚拟桌面尺寸为空",
            true,
        ));
    }
    let pixels = wgc::capture_virtual_desktop(desktop_rect, width, height)?;
    let windows = windows_catalog(desktop);
    let frame = CapturedFrame::new(
        pixels,
        width,
        height,
        f64::from(width),
        f64::from(height),
        PixelFormat::Bgra8,
    )?;
    let selection = show_selection_window(
        desktop_rect,
        frame.clone(),
        Size::new(
            options.effective_min_width() * ui_scale,
            options.effective_min_height() * ui_scale,
        ),
        ui_scale,
        windows,
        cursor_desktop_point(desktop),
    )?;
    match selection {
        WindowsSelectionOutcome::Cancelled => Ok(NativeCaptureOutcome::Cancelled),
        WindowsSelectionOutcome::Selected(selection) => {
            Ok(NativeCaptureOutcome::Captured(frame.crop_png(selection)?))
        }
    }
}

fn virtual_desktop_rect() -> Result<RECT> {
    let left = unsafe { GetSystemMetrics(SM_XVIRTUALSCREEN) };
    let top = unsafe { GetSystemMetrics(SM_YVIRTUALSCREEN) };
    let width = unsafe { GetSystemMetrics(SM_CXVIRTUALSCREEN) };
    let height = unsafe { GetSystemMetrics(SM_CYVIRTUALSCREEN) };
    if width <= 0 || height <= 0 {
        return Err(Error::new(
            ErrorCode::DisplayUnavailable,
            "Windows 虚拟桌面尺寸为空",
            true,
        ));
    }
    Ok(RECT {
        left,
        top,
        right: left.saturating_add(width),
        bottom: top.saturating_add(height),
    })
}

fn ui_scale_under_cursor() -> Result<f64> {
    let mut cursor = POINT::default();
    unsafe { GetCursorPos(&mut cursor) }.map_err(|error| {
        Error::new(
            ErrorCode::DisplayUnavailable,
            format!("无法读取 Windows 鼠标位置：{error}"),
            true,
        )
    })?;
    let monitor = unsafe { MonitorFromPoint(cursor, MONITOR_DEFAULTTONEAREST) };
    if monitor.is_invalid() {
        return Err(Error::new(
            ErrorCode::DisplayUnavailable,
            "找不到鼠标所在的 Windows 显示器",
            true,
        ));
    }
    let mut dpi_x = 96;
    let mut dpi_y = 96;
    let _ = unsafe { GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) };
    Ok((f64::from(dpi_x.max(dpi_y)) / 96.0).clamp(0.5, 4.0))
}

fn desktop_rect_from_win32(rect: RECT) -> DesktopRect {
    DesktopRect::new(
        f64::from(rect.left),
        f64::from(rect.top),
        f64::from(rect.right.saturating_sub(rect.left)),
        f64::from(rect.bottom.saturating_sub(rect.top)),
    )
}

fn cursor_desktop_point(desktop: DesktopRect) -> Option<Point> {
    let mut cursor = POINT::default();
    unsafe { GetCursorPos(&mut cursor) }.ok()?;
    desktop.local_point(f64::from(cursor.x), f64::from(cursor.y))
}

fn windows_catalog(desktop: DesktopRect) -> WindowCatalog {
    let mut windows = Vec::<DesktopRect>::new();
    let data = LPARAM((&mut windows as *mut Vec<DesktopRect>) as isize);
    let _ = unsafe { EnumWindows(Some(collect_window_bounds), data) };
    WindowCatalog::from_absolute(desktop, windows)
}

unsafe extern "system" fn collect_window_bounds(hwnd: HWND, data: LPARAM) -> BOOL {
    if !unsafe { IsWindowVisible(hwnd) }.as_bool() {
        return BOOL(1);
    }
    let extended_style = unsafe { GetWindowLongW(hwnd, GWL_EXSTYLE) } as u32;
    if extended_style & WS_EX_TOOLWINDOW.0 != 0 {
        return BOOL(1);
    }

    let mut cloaked = 0u32;
    if unsafe {
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_CLOAKED,
            (&mut cloaked as *mut u32).cast(),
            size_of::<u32>() as u32,
        )
    }
    .is_ok()
        && cloaked != 0
    {
        return BOOL(1);
    }

    let mut rect = RECT::default();
    if unsafe {
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            (&mut rect as *mut RECT).cast(),
            size_of::<RECT>() as u32,
        )
    }
    .is_err()
        && unsafe { GetWindowRect(hwnd, &mut rect) }.is_err()
    {
        return BOOL(1);
    }
    if rect.right <= rect.left || rect.bottom <= rect.top {
        return BOOL(1);
    }

    let windows = unsafe { &mut *(data.0 as *mut Vec<DesktopRect>) };
    windows.push(desktop_rect_from_win32(rect));
    BOOL(1)
}

enum WindowsSelectionOutcome {
    Selected(Rect),
    Cancelled,
}

struct WindowState {
    frame: CapturedFrame,
    dimmed: Vec<u8>,
    renderer: Option<Direct2DOverlay>,
    model: SelectionModel,
    windows: WindowCatalog,
    desktop: RECT,
    cursor: Option<Point>,
    ui_scale: f64,
    message_loop_started: bool,
    result: Option<WindowsSelectionOutcome>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct VisualSnapshot {
    selection: Option<Rect>,
    toolbar: Option<Rect>,
    magnifier: Option<(Point, Point)>,
}

impl WindowState {
    fn new(
        frame: CapturedFrame,
        min_size: Size,
        ui_scale: f64,
        windows: WindowCatalog,
        cursor: Option<Point>,
        desktop: RECT,
    ) -> Result<Self> {
        let mut dimmed = frame.pixels().as_ref().clone();
        for pixel in dimmed.chunks_exact_mut(4) {
            pixel[0] = (u16::from(pixel[0]) * 58 / 100) as u8;
            pixel[1] = (u16::from(pixel[1]) * 58 / 100) as u8;
            pixel[2] = (u16::from(pixel[2]) * 58 / 100) as u8;
            pixel[3] = 255;
        }
        let mut model = SelectionModel::new_with_scale(
            Size::new(
                f64::from(frame.pixel_width()),
                f64::from(frame.pixel_height()),
            ),
            min_size,
            ui_scale,
        );
        if let Some(cursor) = cursor {
            let _ = model.hover_window(windows.window_at(cursor));
        }
        Ok(Self {
            renderer: None,
            model,
            windows,
            desktop,
            cursor,
            frame,
            dimmed,
            ui_scale,
            message_loop_started: false,
            result: None,
        })
    }

    fn visual_snapshot(&self) -> VisualSnapshot {
        VisualSnapshot {
            selection: self.model.preview_selection(),
            toolbar: self.model.toolbar().map(|toolbar| toolbar.frame),
            magnifier: self.cursor.and_then(|cursor| {
                self.model
                    .magnifier_focus(cursor)
                    .map(|focus| (cursor, focus))
            }),
        }
    }
}

fn show_selection_window(
    desktop: RECT,
    frame: CapturedFrame,
    min_size: Size,
    ui_scale: f64,
    windows: WindowCatalog,
    cursor: Option<Point>,
) -> Result<WindowsSelectionOutcome> {
    let instance = module_instance()?;
    register_window_class(instance)?;
    let mut state = Box::new(WindowState::new(
        frame, min_size, ui_scale, windows, cursor, desktop,
    )?);
    let state_ptr = (&mut *state as *mut WindowState).cast::<c_void>();
    let class_name = wide(CLASS_NAME);
    #[cfg(test)]
    let extended_style = WS_EX_TOPMOST;
    #[cfg(not(test))]
    let extended_style = WS_EX_TOOLWINDOW | WS_EX_TOPMOST;
    let hwnd = unsafe {
        CreateWindowExW(
            extended_style,
            PCWSTR(class_name.as_ptr()),
            PCWSTR(class_name.as_ptr()),
            WS_POPUP,
            desktop.left,
            desktop.top,
            desktop.right.saturating_sub(desktop.left),
            desktop.bottom.saturating_sub(desktop.top),
            None,
            Some(HMENU::default()),
            Some(instance),
            Some(state_ptr),
        )
    }
    .map_err(|error| {
        Error::new(
            ErrorCode::OverlayFailed,
            format!("创建 Windows 截图浮层失败：{error}"),
            true,
        )
    })?;
    unsafe {
        SetWindowPos(
            hwnd,
            Some(HWND_TOPMOST),
            desktop.left,
            desktop.top,
            desktop.right.saturating_sub(desktop.left),
            desktop.bottom.saturating_sub(desktop.top),
            SWP_NOOWNERZORDER,
        )
    }
    .map_err(|error| {
        let _ = unsafe { DestroyWindow(hwnd) };
        Error::new(
            ErrorCode::OverlayFailed,
            format!("定位 Windows 截图浮层失败：{error}"),
            true,
        )
    })?;
    state.message_loop_started = true;
    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetForegroundWindow(hwnd);
        let _ = SetFocus(Some(hwnd));
        let _ = InvalidateRect(Some(hwnd), None, false);
    }

    let mut message = MSG::default();
    loop {
        let status = unsafe { GetMessageW(&mut message, None, 0, 0) };
        if status.0 == -1 {
            state.message_loop_started = false;
            let _ = unsafe { DestroyWindow(hwnd) };
            return Err(Error::new(
                ErrorCode::OverlayFailed,
                "Windows 截图浮层消息循环失败",
                true,
            ));
        }
        if status.0 == 0 {
            break;
        }
        unsafe {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }

    Ok(state
        .result
        .take()
        .unwrap_or(WindowsSelectionOutcome::Cancelled))
}

fn module_instance() -> Result<HINSTANCE> {
    unsafe { GetModuleHandleW(None) }
        .map(|module| HINSTANCE(module.0))
        .map_err(|error| {
            Error::new(
                ErrorCode::OverlayFailed,
                format!("无法读取 Windows 应用模块句柄：{error}"),
                true,
            )
        })
}

fn register_window_class(instance: HINSTANCE) -> Result<()> {
    let class_name = wide(CLASS_NAME);
    let cursor = unsafe { LoadCursorW(None, IDC_CROSS) }.unwrap_or_default();
    let class = WNDCLASSW {
        style: CS_HREDRAW | CS_VREDRAW | CS_DBLCLKS,
        lpfnWndProc: Some(window_proc),
        hInstance: instance,
        hCursor: cursor,
        lpszClassName: PCWSTR(class_name.as_ptr()),
        ..Default::default()
    };
    let atom = unsafe { RegisterClassW(&class) };
    if atom == 0 && unsafe { GetLastError() } != ERROR_CLASS_ALREADY_EXISTS {
        return Err(Error::new(
            ErrorCode::OverlayFailed,
            "注册 Windows 截图浮层窗口类失败",
            true,
        ));
    }
    Ok(())
}

extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if message == WM_NCCREATE {
        let create = unsafe { &*(lparam.0 as *const CREATESTRUCTW) };
        unsafe {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize);
        }
        return LRESULT(1);
    }

    let state_ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *mut WindowState;
    if state_ptr.is_null() {
        return unsafe { DefWindowProcW(hwnd, message, wparam, lparam) };
    }
    let state = unsafe { &mut *state_ptr };

    match message {
        WM_PAINT => {
            paint_window(hwnd, state);
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_SETCURSOR => {
            set_cursor_for_state(state);
            LRESULT(1)
        }
        WM_LBUTTONDOWN => {
            let previous = state.visual_snapshot();
            let point = point_from_lparam(lparam);
            state.cursor = Some(point);
            let interaction = state.model.pointer_down(point);
            if !apply_interaction(hwnd, state, interaction, previous) {
                unsafe {
                    SetCapture(hwnd);
                }
            }
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            let previous = state.visual_snapshot();
            let point = point_from_lparam(lparam);
            state.cursor = Some(point);
            let mut interaction = state.model.pointer_move(point);
            if interaction == Interaction::Unchanged {
                interaction = state.model.hover_window(state.windows.window_at(point));
            }
            apply_interaction(hwnd, state, interaction, previous);
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            let previous = state.visual_snapshot();
            unsafe {
                let _ = ReleaseCapture();
            }
            let point = point_from_lparam(lparam);
            state.cursor = Some(point);
            let interaction = state.model.pointer_up(point);
            apply_interaction(hwnd, state, interaction, previous);
            LRESULT(0)
        }
        WM_LBUTTONDBLCLK => {
            let previous = state.visual_snapshot();
            let interaction = state.model.confirm();
            apply_interaction(hwnd, state, interaction, previous);
            LRESULT(0)
        }
        WM_RBUTTONDOWN | WM_CLOSE | WM_DISPLAYCHANGE => {
            let previous = state.visual_snapshot();
            apply_interaction(hwnd, state, Interaction::Cancel, previous);
            LRESULT(0)
        }
        WM_KEYDOWN => {
            let previous = state.visual_snapshot();
            let interaction = if wparam.0 == VK_ESCAPE.0 as usize {
                state.model.cancel()
            } else if wparam.0 == VK_RETURN.0 as usize {
                state.model.confirm()
            } else {
                Interaction::Unchanged
            };
            apply_interaction(hwnd, state, interaction, previous);
            LRESULT(0)
        }
        WM_DESTROY => {
            unsafe {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                if state.message_loop_started {
                    PostQuitMessage(0);
                }
            }
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
    }
}

fn apply_interaction(
    hwnd: HWND,
    state: &mut WindowState,
    interaction: Interaction,
    previous: VisualSnapshot,
) -> bool {
    match interaction {
        Interaction::Confirm(selection) => {
            state.result = Some(WindowsSelectionOutcome::Selected(selection));
            let _ = unsafe { DestroyWindow(hwnd) };
            true
        }
        Interaction::Cancel => {
            state.result = Some(WindowsSelectionOutcome::Cancelled);
            let _ = unsafe { DestroyWindow(hwnd) };
            true
        }
        Interaction::Changed | Interaction::Unchanged => {
            set_cursor_for_state(state);
            invalidate_visual_delta(hwnd, state, previous);
            false
        }
    }
}

fn set_cursor_for_state(state: &WindowState) {
    let cursor_name = if state.model.magnifier_visible() {
        IDC_CROSS
    } else {
        IDC_ARROW
    };
    if let Ok(cursor) = unsafe { LoadCursorW(None, cursor_name) } {
        unsafe {
            SetCursor(Some(cursor));
        }
    }
}

fn invalidate_visual_delta(hwnd: HWND, state: &WindowState, previous: VisualSnapshot) {
    let current = state.visual_snapshot();
    if previous != current {
        unsafe {
            let _ = InvalidateRect(Some(hwnd), None, false);
        }
    }
}

fn paint_window(hwnd: HWND, state: &mut WindowState) {
    if state.renderer.is_none() {
        match Direct2DOverlay::new(hwnd, &state.frame, &state.dimmed) {
            Ok(renderer) => state.renderer = Some(renderer),
            Err(_) => {
                state.result = Some(WindowsSelectionOutcome::Cancelled);
                let _ = unsafe { DestroyWindow(hwnd) };
                return;
            }
        }
    }

    let magnifier = state.cursor.and_then(|cursor| {
        let focus = state.model.magnifier_focus(cursor)?;
        let (viewport, ui_scale) = monitor_viewport(state, cursor)?;
        layout_magnifier(
            cursor,
            focus,
            viewport,
            Size::new(
                f64::from(state.frame.pixel_width()),
                f64::from(state.frame.pixel_height()),
            ),
            ui_scale,
        )
    });
    let scene = OverlayScene {
        selection: state.model.preview_selection(),
        show_handles: state.model.selection().is_some(),
        toolbar: state.model.toolbar().map(|toolbar| toolbar.frame),
        magnifier,
        ui_scale: state.ui_scale,
    };
    let frame_size = Size::new(
        f64::from(state.frame.pixel_width()),
        f64::from(state.frame.pixel_height()),
    );
    if state
        .renderer
        .as_ref()
        .is_some_and(|renderer| renderer.render(&scene, frame_size).is_err())
    {
        state.renderer = None;
        unsafe {
            let _ = InvalidateRect(Some(hwnd), None, false);
        }
    }
    unsafe {
        let _ = ValidateRect(Some(hwnd), None);
    }
}

fn monitor_viewport(state: &WindowState, cursor: Point) -> Option<(Rect, f64)> {
    let screen_cursor = POINT {
        x: state.desktop.left.saturating_add(cursor.x.round() as i32),
        y: state.desktop.top.saturating_add(cursor.y.round() as i32),
    };
    let monitor = unsafe { MonitorFromPoint(screen_cursor, MONITOR_DEFAULTTONEAREST) };
    if monitor.is_invalid() {
        return None;
    }
    let mut info = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    if !unsafe { GetMonitorInfoW(monitor, &mut info) }.as_bool() {
        return None;
    }
    let mut dpi_x = 96;
    let mut dpi_y = 96;
    let _ = unsafe { GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) };
    let ui_scale = (f64::from(dpi_x.max(dpi_y)) / 96.0).clamp(0.5, 4.0);
    Some((
        Rect {
            x: f64::from(info.rcMonitor.left.saturating_sub(state.desktop.left)),
            y: f64::from(info.rcMonitor.top.saturating_sub(state.desktop.top)),
            width: f64::from(info.rcMonitor.right.saturating_sub(info.rcMonitor.left)),
            height: f64::from(info.rcMonitor.bottom.saturating_sub(info.rcMonitor.top)),
        },
        ui_scale,
    ))
}

fn point_from_lparam(lparam: LPARAM) -> Point {
    let x = (lparam.0 as u32 & 0xffff) as u16 as i16;
    let y = ((lparam.0 as u32 >> 16) & 0xffff) as u16 as i16;
    Point::new(f64::from(x), f64::from(y))
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(test)]
mod interactive_tests {
    use super::*;

    #[test]
    #[ignore = "requires an interactive Windows desktop"]
    fn wgc_direct2d_selection_smoke_test() {
        let outcome = run_native_capture(CaptureOptions::default())
            .expect("interactive WGC/Direct2D capture should complete");
        assert!(matches!(
            outcome,
            NativeCaptureOutcome::Captured(_) | NativeCaptureOutcome::Cancelled
        ));
    }
}
