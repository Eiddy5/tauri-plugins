#[cfg(windows)]
mod windows;

#[cfg(windows)]
pub use windows::{
    windows_display_index_from_source_id, windows_target_handle_from_source_id, WindowsShareOverlay,
};

use async_trait::async_trait;
use std::sync::Arc;

use crate::{models::CaptureSourceKind, Result};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OverlayTarget {
    pub source_id: String,
    pub source_kind: CaptureSourceKind,
}

#[async_trait]
pub trait ShareOverlay: Send + Sync {
    async fn start(&self, target: OverlayTarget) -> Result<()>;
    async fn show(&self) -> Result<()>;
    async fn hide(&self) -> Result<()>;
    async fn stop(&self) -> Result<()>;
}

pub trait ShareOverlayFactory: Send + Sync {
    fn create_overlay(&self) -> Arc<dyn ShareOverlay>;
}

#[derive(Debug, Default)]
pub struct NoopShareOverlay;

#[async_trait]
impl ShareOverlay for NoopShareOverlay {
    async fn start(&self, _target: OverlayTarget) -> Result<()> {
        Ok(())
    }

    async fn show(&self) -> Result<()> {
        Ok(())
    }

    async fn hide(&self) -> Result<()> {
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        Ok(())
    }
}

#[derive(Debug, Default)]
pub struct NoopShareOverlayFactory;

impl ShareOverlayFactory for NoopShareOverlayFactory {
    fn create_overlay(&self) -> Arc<dyn ShareOverlay> {
        Arc::new(NoopShareOverlay)
    }
}

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
    let thickness = style.thickness.max(1);
    let corner_length = style.corner_length.max(thickness);
    let min_dimension = thickness.saturating_mul(2);
    let width = rect.right.saturating_sub(rect.left).max(min_dimension);
    let height = rect.bottom.saturating_sub(rect.top).max(min_dimension);
    let horizontal_length = corner_length.min(width / 2);
    let vertical_length = corner_length.min(height / 2);

    vec![
        OverlaySegment::new(rect.left, rect.top, horizontal_length, thickness),
        OverlaySegment::new(rect.left, rect.top, thickness, vertical_length),
        OverlaySegment::new(
            rect.right.saturating_sub(horizontal_length),
            rect.top,
            horizontal_length,
            thickness,
        ),
        OverlaySegment::new(
            rect.right.saturating_sub(thickness),
            rect.top,
            thickness,
            vertical_length,
        ),
        OverlaySegment::new(
            rect.left,
            rect.bottom.saturating_sub(thickness),
            horizontal_length,
            thickness,
        ),
        OverlaySegment::new(
            rect.left,
            rect.bottom.saturating_sub(vertical_length),
            thickness,
            vertical_length,
        ),
        OverlaySegment::new(
            rect.right.saturating_sub(horizontal_length),
            rect.bottom.saturating_sub(thickness),
            horizontal_length,
            thickness,
        ),
        OverlaySegment::new(
            rect.right.saturating_sub(thickness),
            rect.bottom.saturating_sub(vertical_length),
            thickness,
            vertical_length,
        ),
    ]
}
