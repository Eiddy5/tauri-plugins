use crate::{CaptureErrorCode, Error, Result};

use super::NormalizedPoint;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

impl Point {
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Size {
    pub width: f64,
    pub height: f64,
}

impl Size {
    pub const fn new(width: f64, height: f64) -> Self {
        Self { width, height }
    }

    fn is_valid(self) -> bool {
        self.width.is_finite() && self.height.is_finite() && self.width > 0.0 && self.height > 0.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Rect {
    pub const fn new(x: f64, y: f64, width: f64, height: f64) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    fn is_valid(self) -> bool {
        self.x.is_finite() && self.y.is_finite() && Size::new(self.width, self.height).is_valid()
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CaptureGeometry {
    overlay_rect: Rect,
    output_content_rect: Rect,
}

impl CaptureGeometry {
    pub fn aspect_fit(source: Size, output: Size) -> Self {
        Self::try_aspect_fit(source, output).expect("capture sizes must be finite and positive")
    }

    pub fn try_aspect_fit(source: Size, output: Size) -> Result<Self> {
        if !source.is_valid() || !output.is_valid() {
            return Err(invalid_geometry());
        }
        let scale = (output.width / source.width).min(output.height / source.height);
        let width = source.width * scale;
        let height = source.height * scale;
        let content = Rect::new(
            (output.width - width) / 2.0,
            (output.height - height) / 2.0,
            width,
            height,
        );
        Ok(Self {
            overlay_rect: Rect::new(0.0, 0.0, source.width, source.height),
            output_content_rect: content,
        })
    }

    pub fn from_overlay(overlay_rect: Rect) -> Self {
        Self::try_from_overlay(overlay_rect).expect("overlay rectangle must be finite and positive")
    }

    pub fn try_from_overlay(overlay_rect: Rect) -> Result<Self> {
        if !overlay_rect.is_valid() {
            return Err(invalid_geometry());
        }
        Ok(Self {
            overlay_rect,
            output_content_rect: Rect::new(0.0, 0.0, overlay_rect.width, overlay_rect.height),
        })
    }

    pub fn output_content_rect(self) -> Rect {
        self.output_content_rect
    }

    pub fn to_output(self, point: NormalizedPoint) -> Point {
        Point::new(
            self.output_content_rect.x + f64::from(point.x) * self.output_content_rect.width,
            self.output_content_rect.y + f64::from(point.y) * self.output_content_rect.height,
        )
    }

    pub fn overlay_to_normalized(self, point: Point) -> NormalizedPoint {
        NormalizedPoint::new(
            ((point.x - self.overlay_rect.x) / self.overlay_rect.width) as f32,
            ((self.overlay_rect.y + self.overlay_rect.height - point.y) / self.overlay_rect.height)
                as f32,
        )
    }
}

fn invalid_geometry() -> Error {
    Error::new(
        CaptureErrorCode::AnnotationInvalidOptions,
        "标注坐标范围必须是有限的正尺寸矩形",
        true,
    )
}
