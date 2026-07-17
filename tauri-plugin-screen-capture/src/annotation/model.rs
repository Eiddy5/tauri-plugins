use std::sync::Arc;

use crate::{CaptureErrorCode, Error, Result};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NormalizedPoint {
    pub x: f32,
    pub y: f32,
}

impl NormalizedPoint {
    pub fn new(x: f32, y: f32) -> Self {
        Self {
            x: x.clamp(0.0, 1.0),
            y: y.clamp(0.0, 1.0),
        }
    }

    pub(crate) fn distance(self, other: Self) -> f32 {
        (self.x - other.x).hypot(self.y - other.y)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AnnotationRgba {
    pub red: u8,
    pub green: u8,
    pub blue: u8,
    pub alpha: u8,
}

impl AnnotationRgba {
    pub fn parse(value: &str) -> Result<Self> {
        let digits = value.strip_prefix('#').ok_or_else(invalid_color)?;
        if !matches!(digits.len(), 6 | 8) || !digits.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(invalid_color());
        }
        let channel =
            |start| u8::from_str_radix(&digits[start..start + 2], 16).map_err(|_| invalid_color());
        Ok(Self {
            red: channel(0)?,
            green: channel(2)?,
            blue: channel(4)?,
            alpha: if digits.len() == 8 { channel(6)? } else { 255 },
        })
    }
}

fn invalid_color() -> Error {
    Error::new(
        CaptureErrorCode::AnnotationInvalidOptions,
        "标注颜色必须是 #RRGGBB 或 #RRGGBBAA",
        true,
    )
}

#[derive(Clone, Debug)]
pub struct Stroke {
    points: Arc<[NormalizedPoint]>,
    normalized_width: f32,
    color: Option<AnnotationRgba>,
}

impl Stroke {
    pub fn new(
        points: Vec<NormalizedPoint>,
        normalized_width: f32,
        color: Option<AnnotationRgba>,
    ) -> Result<Self> {
        if points.is_empty()
            || !normalized_width.is_finite()
            || normalized_width <= 0.0
            || points
                .iter()
                .any(|point| !point.x.is_finite() || !point.y.is_finite())
        {
            return Err(Error::new(
                CaptureErrorCode::AnnotationInvalidOptions,
                "标注笔迹包含无效数据",
                true,
            ));
        }
        Ok(Self {
            points: points.into(),
            normalized_width,
            color,
        })
    }

    pub fn points(&self) -> &[NormalizedPoint] {
        &self.points
    }

    pub fn normalized_width(&self) -> f32 {
        self.normalized_width
    }

    pub fn color(&self) -> Option<AnnotationRgba> {
        self.color
    }
}

#[derive(Clone, Debug)]
pub enum AnnotationOperation {
    Pen(Stroke),
    Eraser(Stroke),
}

impl AnnotationOperation {
    pub fn is_eraser(&self) -> bool {
        matches!(self, Self::Eraser(_))
    }

    pub fn stroke(&self) -> &Stroke {
        match self {
            Self::Pen(stroke) | Self::Eraser(stroke) => stroke,
        }
    }
}

#[derive(Clone, Debug)]
pub struct AnnotationSnapshot {
    revision: u64,
    operations: Arc<[AnnotationOperation]>,
}

impl AnnotationSnapshot {
    pub(crate) fn new(revision: u64, operations: Vec<AnnotationOperation>) -> Self {
        Self {
            revision,
            operations: operations.into(),
        }
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn operations(&self) -> &[AnnotationOperation] {
        &self.operations
    }
}
