#[cfg(windows)]
mod windows;

#[cfg(windows)]
pub use windows::WindowsShareOverlay;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OverlayStyle {
    pub color: u32,
    pub thickness: i32,
    pub corner_length: i32,
}

impl Default for OverlayStyle {
    fn default() -> Self {
        Self {
            color: 0x22C55E,
            thickness: 4,
            corner_length: 32,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OverlayRect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OverlaySegment {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

impl OverlaySegment {
    pub const fn new(x: i32, y: i32, width: i32, height: i32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }
}

pub fn corner_segments(rect: OverlayRect, style: OverlayStyle) -> Vec<OverlaySegment> {
    let width = rect.right - rect.left;
    let height = rect.bottom - rect.top;
    let horizontal_length = style.corner_length.min(width / 2);
    let vertical_length = style.corner_length.min(height / 2);

    vec![
        OverlaySegment::new(rect.left, rect.top, horizontal_length, style.thickness),
        OverlaySegment::new(rect.left, rect.top, style.thickness, vertical_length),
        OverlaySegment::new(
            rect.right - horizontal_length,
            rect.top,
            horizontal_length,
            style.thickness,
        ),
        OverlaySegment::new(
            rect.right - style.thickness,
            rect.top,
            style.thickness,
            vertical_length,
        ),
        OverlaySegment::new(
            rect.left,
            rect.bottom - style.thickness,
            horizontal_length,
            style.thickness,
        ),
        OverlaySegment::new(
            rect.left,
            rect.bottom - vertical_length,
            style.thickness,
            vertical_length,
        ),
        OverlaySegment::new(
            rect.right - horizontal_length,
            rect.bottom - style.thickness,
            horizontal_length,
            style.thickness,
        ),
        OverlaySegment::new(
            rect.right - style.thickness,
            rect.bottom - vertical_length,
            style.thickness,
            vertical_length,
        ),
    ]
}
