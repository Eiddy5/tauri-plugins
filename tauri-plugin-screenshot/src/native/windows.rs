use std::{ffi::c_void, mem::size_of};

use async_trait::async_trait;
use windows::{
    core::{BOOL, PCWSTR},
    Win32::{
        Foundation::{
            GetLastError, COLORREF, ERROR_CLASS_ALREADY_EXISTS, HWND, LPARAM, LRESULT, POINT, RECT,
            WPARAM,
        },
        Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS},
        Graphics::Gdi::{
            BeginPaint, BitBlt, CreateCompatibleDC, CreateDIBSection, CreatePen, CreateSolidBrush,
            DeleteDC, DeleteObject, EndPaint, GetDC, GetMonitorInfoW, GetStockObject,
            InvalidateRect, LineTo, MonitorFromPoint, MoveToEx, Rectangle, ReleaseDC, RoundRect,
            SelectObject, SetStretchBltMode, StretchDIBits, BITMAPINFO, BITMAPINFOHEADER, BI_RGB,
            CAPTUREBLT, COLORONCOLOR, DIB_RGB_COLORS, HBITMAP, HGDIOBJ, HOLLOW_BRUSH, MONITORINFO,
            MONITOR_DEFAULTTONEAREST, PAINTSTRUCT, PS_SOLID, SRCCOPY,
        },
        UI::{
            HiDpi::{
                GetDpiForMonitor, SetThreadDpiAwarenessContext,
                DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, MDT_EFFECTIVE_DPI,
            },
            Input::KeyboardAndMouse::{ReleaseCapture, SetCapture, SetFocus, VK_ESCAPE, VK_RETURN},
            WindowsAndMessaging::{
                CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, EnumWindows,
                GetCursorPos, GetMessageW, GetSystemMetrics, GetWindowLongPtrW, GetWindowLongW,
                GetWindowRect, IsWindowVisible, LoadCursorW, PostQuitMessage, RegisterClassW,
                SetForegroundWindow, SetWindowLongPtrW, SetWindowPos, ShowWindow, TranslateMessage,
                CREATESTRUCTW, CS_DBLCLKS, CS_HREDRAW, CS_VREDRAW, GWLP_USERDATA, GWL_EXSTYLE,
                HMENU, HWND_TOPMOST, IDC_CROSS, MSG, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN,
                SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, SWP_NOOWNERZORDER, SW_SHOW, WM_CLOSE,
                WM_DESTROY, WM_DISPLAYCHANGE, WM_ERASEBKGND, WM_KEYDOWN, WM_LBUTTONDBLCLK,
                WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_NCCREATE, WM_PAINT, WM_RBUTTONDOWN,
                WNDCLASSW, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
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

use super::{
    desktop::{DesktopRect, WindowCatalog},
    magnifier::{layout_magnifier, MagnifierLayout},
    CapturedFrame, NativeCaptureAdapter, NativeCaptureOutcome, PixelFormat,
};

const CLASS_NAME: &str = "TauriPluginNativeScreenshotOverlay";
const HANDLE_SIZE: f64 = 8.0;

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
    let previous_dpi =
        unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
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
    let pixels = capture_desktop_bgra(desktop_rect, width, height)?;
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

fn capture_desktop_bgra(rect: RECT, width: u32, height: u32) -> Result<Vec<u8>> {
    let screen_dc = unsafe { GetDC(None) };
    if screen_dc.is_invalid() {
        return Err(capture_error("GetDC 返回无效句柄"));
    }
    let memory_dc = unsafe { CreateCompatibleDC(Some(screen_dc)) };
    if memory_dc.is_invalid() {
        unsafe {
            ReleaseDC(None, screen_dc);
        }
        return Err(capture_error("CreateCompatibleDC 返回无效句柄"));
    }

    let mut bits = std::ptr::null_mut();
    let bitmap_info = top_down_bitmap_info(width, height)?;
    let bitmap = match unsafe {
        CreateDIBSection(
            Some(screen_dc),
            &bitmap_info,
            DIB_RGB_COLORS,
            &mut bits,
            None,
            0,
        )
    } {
        Ok(bitmap) => bitmap,
        Err(error) => {
            cleanup_capture_dc(screen_dc, memory_dc, None, None);
            return Err(capture_error(format!("CreateDIBSection 失败：{error}")));
        }
    };
    let old_object = unsafe { SelectObject(memory_dc, HGDIOBJ(bitmap.0)) };
    let capture = unsafe {
        BitBlt(
            memory_dc,
            0,
            0,
            width as i32,
            height as i32,
            Some(screen_dc),
            rect.left,
            rect.top,
            SRCCOPY | CAPTUREBLT,
        )
    };
    let result = match capture {
        Err(error) => Err(capture_error(format!("BitBlt 截图失败：{error}"))),
        Ok(()) if bits.is_null() => Err(capture_error("截图像素指针为空")),
        Ok(()) => {
            let len = width as usize * height as usize * 4;
            let source = unsafe { std::slice::from_raw_parts(bits.cast::<u8>(), len) };
            let mut pixels = source.to_vec();
            for pixel in pixels.chunks_exact_mut(4) {
                pixel[3] = 255;
            }
            Ok(pixels)
        }
    };
    cleanup_capture_dc(screen_dc, memory_dc, Some(bitmap), Some(old_object));
    result
}

fn cleanup_capture_dc(
    screen_dc: windows::Win32::Graphics::Gdi::HDC,
    memory_dc: windows::Win32::Graphics::Gdi::HDC,
    bitmap: Option<HBITMAP>,
    old_object: Option<HGDIOBJ>,
) {
    unsafe {
        if let Some(old_object) = old_object {
            SelectObject(memory_dc, old_object);
        }
        if let Some(bitmap) = bitmap {
            let _ = DeleteObject(HGDIOBJ(bitmap.0));
        }
        let _ = DeleteDC(memory_dc);
        ReleaseDC(None, screen_dc);
    }
}

fn top_down_bitmap_info(width: u32, height: u32) -> Result<BITMAPINFO> {
    Ok(BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: i32::try_from(width).map_err(|_| capture_error("截图宽度超出 Win32 范围"))?,
            biHeight: -i32::try_from(height)
                .map_err(|_| capture_error("截图高度超出 Win32 范围"))?,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    })
}

enum WindowsSelectionOutcome {
    Selected(Rect),
    Cancelled,
}

struct WindowState {
    frame: CapturedFrame,
    dimmed: Vec<u8>,
    bitmap_info: BITMAPINFO,
    model: SelectionModel,
    windows: WindowCatalog,
    desktop: RECT,
    cursor: Option<Point>,
    ui_scale: f64,
    result: Option<WindowsSelectionOutcome>,
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
            bitmap_info: top_down_bitmap_info(frame.pixel_width(), frame.pixel_height())?,
            model,
            windows,
            desktop,
            cursor,
            frame,
            dimmed,
            ui_scale,
            result: None,
        })
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
    register_window_class()?;
    let mut state = Box::new(WindowState::new(
        frame, min_size, ui_scale, windows, cursor, desktop,
    )?);
    let state_ptr = (&mut *state as *mut WindowState).cast::<c_void>();
    let class_name = wide(CLASS_NAME);
    let hwnd = unsafe {
        CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
            PCWSTR(class_name.as_ptr()),
            PCWSTR(class_name.as_ptr()),
            WS_POPUP,
            desktop.left,
            desktop.top,
            desktop.right.saturating_sub(desktop.left),
            desktop.bottom.saturating_sub(desktop.top),
            None,
            Some(HMENU::default()),
            None,
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
        Error::new(
            ErrorCode::OverlayFailed,
            format!("定位 Windows 截图浮层失败：{error}"),
            true,
        )
    })?;
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

fn register_window_class() -> Result<()> {
    let class_name = wide(CLASS_NAME);
    let cursor = unsafe { LoadCursorW(None, IDC_CROSS) }.unwrap_or_default();
    let class = WNDCLASSW {
        style: CS_HREDRAW | CS_VREDRAW | CS_DBLCLKS,
        lpfnWndProc: Some(window_proc),
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
        WM_LBUTTONDOWN => {
            let point = point_from_lparam(lparam);
            state.cursor = Some(point);
            let interaction = state.model.pointer_down(point);
            if !apply_interaction(hwnd, state, interaction) {
                unsafe {
                    SetCapture(hwnd);
                }
            }
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            let point = point_from_lparam(lparam);
            let previous_cursor = state.cursor.replace(point);
            let mut interaction = state.model.pointer_move(point);
            if interaction == Interaction::Unchanged {
                interaction = state.model.hover_window(state.windows.window_at(point));
            }
            if interaction == Interaction::Unchanged && state.model.magnifier_visible() {
                invalidate_magnifier(hwnd, state, previous_cursor, Some(point));
                return LRESULT(0);
            }
            apply_interaction(hwnd, state, interaction);
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            unsafe {
                let _ = ReleaseCapture();
            }
            let point = point_from_lparam(lparam);
            state.cursor = Some(point);
            let interaction = state.model.pointer_up(point);
            apply_interaction(hwnd, state, interaction);
            LRESULT(0)
        }
        WM_LBUTTONDBLCLK => {
            let interaction = state.model.confirm();
            apply_interaction(hwnd, state, interaction);
            LRESULT(0)
        }
        WM_RBUTTONDOWN | WM_CLOSE | WM_DISPLAYCHANGE => {
            apply_interaction(hwnd, state, Interaction::Cancel);
            LRESULT(0)
        }
        WM_KEYDOWN => {
            let interaction = if wparam.0 == VK_ESCAPE.0 as usize {
                state.model.cancel()
            } else if wparam.0 == VK_RETURN.0 as usize {
                state.model.confirm()
            } else {
                Interaction::Unchanged
            };
            apply_interaction(hwnd, state, interaction);
            LRESULT(0)
        }
        WM_DESTROY => {
            unsafe {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                PostQuitMessage(0);
            }
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
    }
}

fn apply_interaction(hwnd: HWND, state: &mut WindowState, interaction: Interaction) -> bool {
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
        Interaction::Changed => {
            unsafe {
                let _ = InvalidateRect(Some(hwnd), None, false);
            }
            false
        }
        Interaction::Unchanged => false,
    }
}

fn paint_window(hwnd: HWND, state: &WindowState) {
    let mut paint = PAINTSTRUCT::default();
    let dc = unsafe { BeginPaint(hwnd, &mut paint) };
    if dc.is_invalid() {
        return;
    }
    let width = state.frame.pixel_width() as i32;
    let height = state.frame.pixel_height() as i32;
    let selection = state.model.preview_selection();
    let has_committed_selection = state.model.selection().is_some();
    let original = state.frame.pixels();
    let base = if selection.is_some() {
        original.as_slice()
    } else {
        state.dimmed.as_slice()
    };
    blit_rect(dc, base, &state.bitmap_info, 0, 0, width, height);
    if let Some(selection) = selection {
        for rect in dim_rects(selection, f64::from(width), f64::from(height)) {
            blit_geometry_rect(dc, &state.dimmed, &state.bitmap_info, rect);
        }
        draw_selection(dc, selection, state.ui_scale, has_committed_selection);
    }
    if let Some(toolbar) = state.model.toolbar() {
        draw_toolbar(dc, toolbar.frame, state.ui_scale);
    }
    if state.model.magnifier_visible() {
        draw_magnifier(dc, state);
    }
    unsafe {
        let _ = EndPaint(hwnd, &paint);
    }
}

fn blit_geometry_rect(
    dc: windows::Win32::Graphics::Gdi::HDC,
    pixels: &[u8],
    bitmap_info: &BITMAPINFO,
    rect: Rect,
) {
    let rect = rect_to_i32(rect);
    blit_rect(
        dc,
        pixels,
        bitmap_info,
        rect.left,
        rect.top,
        rect.right - rect.left,
        rect.bottom - rect.top,
    );
}

fn blit_rect(
    dc: windows::Win32::Graphics::Gdi::HDC,
    pixels: &[u8],
    bitmap_info: &BITMAPINFO,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
) {
    if width <= 0 || height <= 0 {
        return;
    }
    unsafe {
        StretchDIBits(
            dc,
            x,
            y,
            width,
            height,
            x,
            y,
            width,
            height,
            Some(pixels.as_ptr().cast()),
            bitmap_info,
            DIB_RGB_COLORS,
            SRCCOPY,
        );
    }
}

fn invalidate_magnifier(
    hwnd: HWND,
    state: &WindowState,
    previous: Option<Point>,
    current: Option<Point>,
) {
    if previous == current {
        return;
    }
    for cursor in [previous, current].into_iter().flatten() {
        let Some(rect) = magnifier_dirty_rect(state, cursor) else {
            continue;
        };
        unsafe {
            let _ = InvalidateRect(Some(hwnd), Some(&rect), false);
        }
    }
}

fn magnifier_dirty_rect(state: &WindowState, cursor: Point) -> Option<RECT> {
    let (viewport, ui_scale) = monitor_viewport(state, cursor)?;
    let layout = layout_magnifier(
        cursor,
        cursor,
        viewport,
        Size::new(
            f64::from(state.frame.pixel_width()),
            f64::from(state.frame.pixel_height()),
        ),
        ui_scale,
    )?;
    let padding = scaled(4.0, ui_scale);
    let mut rect = rect_to_i32(layout.frame);
    rect.left -= padding;
    rect.top -= padding;
    rect.right += padding;
    rect.bottom += padding;
    Some(rect)
}

fn draw_magnifier(dc: windows::Win32::Graphics::Gdi::HDC, state: &WindowState) {
    let Some(cursor) = state.cursor else {
        return;
    };
    let Some(focus) = state.model.magnifier_focus(cursor) else {
        return;
    };
    let Some((viewport, ui_scale)) = monitor_viewport(state, cursor) else {
        return;
    };
    let source_size = Size::new(
        f64::from(state.frame.pixel_width()),
        f64::from(state.frame.pixel_height()),
    );
    let Some(layout) = layout_magnifier(cursor, focus, viewport, source_size, ui_scale) else {
        return;
    };
    draw_magnifier_frame(dc, state, layout, ui_scale);
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

fn draw_magnifier_frame(
    dc: windows::Win32::Graphics::Gdi::HDC,
    state: &WindowState,
    layout: MagnifierLayout,
    ui_scale: f64,
) {
    let frame = rect_to_i32(layout.frame);
    let background = unsafe { CreateSolidBrush(colorref(255, 255, 255)) };
    let background_pen =
        unsafe { CreatePen(PS_SOLID, scaled(1.0, ui_scale), colorref(218, 218, 218)) };
    let old_brush = unsafe { SelectObject(dc, HGDIOBJ(background.0)) };
    let old_pen = unsafe { SelectObject(dc, HGDIOBJ(background_pen.0)) };
    let radius = scaled(8.0, ui_scale);
    unsafe {
        let _ = RoundRect(
            dc,
            frame.left,
            frame.top,
            frame.right,
            frame.bottom,
            radius,
            radius,
        );
        SelectObject(dc, old_brush);
        SelectObject(dc, old_pen);
        let _ = DeleteObject(HGDIOBJ(background.0));
        let _ = DeleteObject(HGDIOBJ(background_pen.0));
    }

    let inset = scaled(3.0, ui_scale);
    let content = RECT {
        left: frame.left + inset,
        top: frame.top + inset,
        right: frame.right - inset,
        bottom: frame.bottom - inset,
    };
    let source = rect_to_i32(layout.sample);
    if content.right > content.left && content.bottom > content.top {
        let previous_mode = unsafe { SetStretchBltMode(dc, COLORONCOLOR) };
        blit_scaled_rect(
            dc,
            state.frame.pixels().as_slice(),
            &state.bitmap_info,
            content,
            source,
        );
        if previous_mode != 0 {
            unsafe {
                SetStretchBltMode(
                    dc,
                    windows::Win32::Graphics::Gdi::STRETCH_BLT_MODE(previous_mode),
                );
            }
        }

        let content_width = f64::from(content.right - content.left);
        let content_height = f64::from(content.bottom - content.top);
        let focus_x = f64::from(content.left)
            + (layout.focus.x - layout.frame.x) / layout.frame.width * content_width;
        let focus_y = f64::from(content.top)
            + (layout.focus.y - layout.frame.y) / layout.frame.height * content_height;
        let crosshair = unsafe { CreatePen(PS_SOLID, scaled(1.0, ui_scale), colorref(7, 193, 96)) };
        let old_pen = unsafe { SelectObject(dc, HGDIOBJ(crosshair.0)) };
        unsafe {
            let _ = MoveToEx(dc, focus_x.round() as i32, content.top, None);
            let _ = LineTo(dc, focus_x.round() as i32, content.bottom);
            let _ = MoveToEx(dc, content.left, focus_y.round() as i32, None);
            let _ = LineTo(dc, content.right, focus_y.round() as i32);
            SelectObject(dc, old_pen);
            let _ = DeleteObject(HGDIOBJ(crosshair.0));
        }
    }
}

fn blit_scaled_rect(
    dc: windows::Win32::Graphics::Gdi::HDC,
    pixels: &[u8],
    bitmap_info: &BITMAPINFO,
    destination: RECT,
    source: RECT,
) {
    let destination_width = destination.right - destination.left;
    let destination_height = destination.bottom - destination.top;
    let source_width = source.right - source.left;
    let source_height = source.bottom - source.top;
    if destination_width <= 0 || destination_height <= 0 || source_width <= 0 || source_height <= 0
    {
        return;
    }
    unsafe {
        StretchDIBits(
            dc,
            destination.left,
            destination.top,
            destination_width,
            destination_height,
            source.left,
            source.top,
            source_width,
            source_height,
            Some(pixels.as_ptr().cast()),
            bitmap_info,
            DIB_RGB_COLORS,
            SRCCOPY,
        );
    }
}

fn rect_to_i32(rect: Rect) -> RECT {
    RECT {
        left: rect.x.round() as i32,
        top: rect.y.round() as i32,
        right: rect.right().round() as i32,
        bottom: rect.bottom().round() as i32,
    }
}

fn dim_rects(selection: Rect, width: f64, height: f64) -> [Rect; 4] {
    [
        Rect {
            x: 0.0,
            y: 0.0,
            width,
            height: selection.y,
        },
        Rect {
            x: 0.0,
            y: selection.bottom(),
            width,
            height: (height - selection.bottom()).max(0.0),
        },
        Rect {
            x: 0.0,
            y: selection.y,
            width: selection.x,
            height: selection.height,
        },
        Rect {
            x: selection.right(),
            y: selection.y,
            width: (width - selection.right()).max(0.0),
            height: selection.height,
        },
    ]
}

fn draw_selection(
    dc: windows::Win32::Graphics::Gdi::HDC,
    selection: Rect,
    ui_scale: f64,
    show_handles: bool,
) {
    let green = colorref(49, 214, 132);
    let pen = unsafe { CreatePen(PS_SOLID, scaled(2.0, ui_scale), green) };
    let old_pen = unsafe { SelectObject(dc, HGDIOBJ(pen.0)) };
    let left = selection.x.round() as i32;
    let top = selection.y.round() as i32;
    let right = selection.right().round() as i32;
    let bottom = selection.bottom().round() as i32;
    unsafe {
        let _ = MoveToEx(dc, left, top, None);
        let _ = LineTo(dc, right, top);
        let _ = LineTo(dc, right, bottom);
        let _ = LineTo(dc, left, bottom);
        let _ = LineTo(dc, left, top);
        SelectObject(dc, old_pen);
        let _ = DeleteObject(HGDIOBJ(pen.0));
    }

    if show_handles {
        let handle_brush = unsafe { CreateSolidBrush(green) };
        let old_brush = unsafe { SelectObject(dc, HGDIOBJ(handle_brush.0)) };
        let hollow_pen = unsafe { GetStockObject(windows::Win32::Graphics::Gdi::NULL_PEN) };
        let old_pen = unsafe { SelectObject(dc, hollow_pen) };
        for (x, y) in [(left, top), (right, top), (left, bottom), (right, bottom)] {
            let half = scaled(HANDLE_SIZE / 2.0, ui_scale);
            unsafe {
                let _ = Rectangle(dc, x - half, y - half, x + half, y + half);
            }
        }
        unsafe {
            SelectObject(dc, old_brush);
            SelectObject(dc, old_pen);
            let _ = DeleteObject(HGDIOBJ(handle_brush.0));
        }
    }
}

fn draw_toolbar(dc: windows::Win32::Graphics::Gdi::HDC, toolbar: Rect, ui_scale: f64) {
    let left = toolbar.x.round() as i32;
    let top = toolbar.y.round() as i32;
    let right = toolbar.right().round() as i32;
    let bottom = toolbar.bottom().round() as i32;
    let brush = unsafe { CreateSolidBrush(colorref(25, 25, 25)) };
    let pen = unsafe { CreatePen(PS_SOLID, 1, colorref(25, 25, 25)) };
    let old_brush = unsafe { SelectObject(dc, HGDIOBJ(brush.0)) };
    let old_pen = unsafe { SelectObject(dc, HGDIOBJ(pen.0)) };
    let radius = scaled(12.0, ui_scale);
    unsafe {
        let _ = RoundRect(dc, left, top, right, bottom, radius, radius);
        SelectObject(dc, old_brush);
        SelectObject(dc, old_pen);
        let _ = DeleteObject(HGDIOBJ(brush.0));
        let _ = DeleteObject(HGDIOBJ(pen.0));
    }

    let white_pen = unsafe { CreatePen(PS_SOLID, scaled(2.0, ui_scale), colorref(245, 245, 245)) };
    let old_pen = unsafe { SelectObject(dc, HGDIOBJ(white_pen.0)) };
    let button_width = (right - left) / 3;
    let mid_y = (top + bottom) / 2;
    let inset = scaled(8.0, ui_scale);
    for index in 1..=2 {
        let x = left + button_width * index;
        unsafe {
            let _ = MoveToEx(dc, x, top + inset, None);
            let _ = LineTo(dc, x, bottom - inset);
        }
    }

    let cancel_x = left + button_width / 2;
    let icon_small = scaled(5.0, ui_scale);
    unsafe {
        let _ = MoveToEx(dc, cancel_x - icon_small, mid_y - icon_small, None);
        let _ = LineTo(dc, cancel_x + icon_small, mid_y + icon_small);
        let _ = MoveToEx(dc, cancel_x + icon_small, mid_y - icon_small, None);
        let _ = LineTo(dc, cancel_x - icon_small, mid_y + icon_small);
    }

    let reset_x = left + button_width + button_width / 2;
    let icon = scaled(6.0, ui_scale);
    let hollow_brush = unsafe { GetStockObject(HOLLOW_BRUSH) };
    let old_brush = unsafe { SelectObject(dc, hollow_brush) };
    unsafe {
        let _ = Rectangle(
            dc,
            reset_x - icon,
            mid_y - icon,
            reset_x + icon,
            mid_y + icon,
        );
        SelectObject(dc, old_brush);
    }

    let confirm_x = left + button_width * 2 + button_width / 2;
    let check_short = scaled(1.0, ui_scale);
    let check_long = scaled(7.0, ui_scale);
    unsafe {
        let _ = MoveToEx(dc, confirm_x - icon, mid_y, None);
        let _ = LineTo(dc, confirm_x - check_short, mid_y + icon_small);
        let _ = LineTo(dc, confirm_x + check_long, mid_y - icon);
        SelectObject(dc, old_pen);
        let _ = DeleteObject(HGDIOBJ(white_pen.0));
    }
}

fn point_from_lparam(lparam: LPARAM) -> Point {
    let x = (lparam.0 as u32 & 0xffff) as u16 as i16;
    let y = ((lparam.0 as u32 >> 16) & 0xffff) as u16 as i16;
    Point::new(f64::from(x), f64::from(y))
}

fn colorref(red: u8, green: u8, blue: u8) -> COLORREF {
    COLORREF(u32::from(red) | (u32::from(green) << 8) | (u32::from(blue) << 16))
}

fn scaled(value: f64, ui_scale: f64) -> i32 {
    (value * ui_scale).round().max(1.0) as i32
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

fn capture_error(message: impl Into<String>) -> Error {
    Error::new(ErrorCode::CaptureFailed, message, true)
}
