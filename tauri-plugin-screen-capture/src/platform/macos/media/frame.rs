use std::sync::Arc;

use apple_cf::cv::CVPixelBuffer;
use screencapturekit::metal::MetalTexture;

use super::surface_pool::SurfaceLease;
use crate::{models::PixelFormat, pipeline::frame::VideoFrame};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FrameRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl FrameRect {
    pub const fn new(x: f64, y: f64, width: f64, height: f64) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    fn is_valid(self) -> bool {
        [self.x, self.y, self.width, self.height]
            .into_iter()
            .all(f64::is_finite)
            && self.width > 0.0
            && self.height > 0.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CaptureFrameGeometry {
    buffer_width: u32,
    buffer_height: u32,
    output_content_rect: FrameRect,
    content_scale: f64,
    scale_factor: f64,
}

impl CaptureFrameGeometry {
    pub fn from_attachments(
        buffer_width: u32,
        buffer_height: u32,
        content_rect: Option<FrameRect>,
        content_scale: Option<f64>,
        scale_factor: Option<f64>,
    ) -> Result<Self, &'static str> {
        if buffer_width == 0 || buffer_height == 0 {
            return Err("capture buffer dimensions must be positive");
        }
        let output_content_rect = content_rect.unwrap_or(FrameRect::new(
            0.0,
            0.0,
            f64::from(buffer_width),
            f64::from(buffer_height),
        ));
        if !output_content_rect.is_valid() {
            return Err("capture content rect is invalid");
        }
        let content_scale = content_scale.unwrap_or(1.0);
        let scale_factor = scale_factor.unwrap_or(1.0);
        if !content_scale.is_finite()
            || content_scale <= 0.0
            || !scale_factor.is_finite()
            || scale_factor <= 0.0
        {
            return Err("capture scale is invalid");
        }
        Ok(Self {
            buffer_width,
            buffer_height,
            output_content_rect,
            content_scale,
            scale_factor,
        })
    }

    pub const fn buffer_size(self) -> (u32, u32) {
        (self.buffer_width, self.buffer_height)
    }

    pub const fn output_content_rect(self) -> FrameRect {
        self.output_content_rect
    }

    pub const fn content_scale(self) -> f64 {
        self.content_scale
    }

    pub const fn scale_factor(self) -> f64 {
        self.scale_factor
    }
}

#[derive(Clone)]
pub struct MacCaptureFrame {
    pixel_buffer: CVPixelBuffer,
    timestamp_ns: u64,
    geometry: CaptureFrameGeometry,
    generation: u64,
}

impl MacCaptureFrame {
    pub fn new(
        pixel_buffer: CVPixelBuffer,
        timestamp_ns: u64,
        geometry: CaptureFrameGeometry,
        generation: u64,
    ) -> Self {
        Self {
            pixel_buffer,
            timestamp_ns,
            geometry,
            generation,
        }
    }

    pub fn pixel_buffer(&self) -> &CVPixelBuffer {
        &self.pixel_buffer
    }

    pub const fn timestamp_ns(&self) -> u64 {
        self.timestamp_ns
    }

    pub const fn geometry(&self) -> CaptureFrameGeometry {
        self.geometry
    }

    pub const fn generation(&self) -> u64 {
        self.generation
    }

    pub fn with_timestamp(&self, timestamp_ns: u64) -> Self {
        Self {
            pixel_buffer: self.pixel_buffer.clone(),
            timestamp_ns,
            geometry: self.geometry,
            generation: self.generation,
        }
    }

    pub fn to_video_frame(&self) -> Result<VideoFrame, &'static str> {
        let guard = self
            .pixel_buffer
            .lock_read_only()
            .map_err(|_| "failed to lock capture pixel buffer")?;
        let width = guard.width();
        let height = guard.height();
        let bytes_per_row = guard.bytes_per_row();
        let tight_bytes_per_row = width
            .checked_mul(4)
            .ok_or("capture frame dimensions overflow")?;
        let source = guard.as_slice();
        if bytes_per_row < tight_bytes_per_row
            || source.len()
                < bytes_per_row
                    .checked_mul(height)
                    .ok_or("capture frame dimensions overflow")?
        {
            return Err("capture pixel buffer layout is invalid");
        }
        let mut data = vec![0; tight_bytes_per_row * height];
        for row in 0..height {
            let source_offset = row * bytes_per_row;
            let destination_offset = row * tight_bytes_per_row;
            data[destination_offset..destination_offset + tight_bytes_per_row]
                .copy_from_slice(&source[source_offset..source_offset + tight_bytes_per_row]);
        }
        Ok(VideoFrame {
            width: u32::try_from(width).unwrap_or(u32::MAX),
            height: u32::try_from(height).unwrap_or(u32::MAX),
            pixel_format: PixelFormat::Bgra,
            timestamp_ns: self.timestamp_ns,
            data: data.into(),
        })
    }
}

#[derive(Clone)]
pub struct MacGpuSurface {
    inner: Arc<MacGpuSurfaceInner>,
    width: u32,
    height: u32,
    timestamp_ns: u64,
}

struct MacGpuSurfaceInner {
    lease: SurfaceLease,
    _textures: Vec<MetalTexture>,
}

impl MacGpuSurface {
    pub(crate) fn new(
        mut lease: SurfaceLease,
        textures: Vec<MetalTexture>,
        timestamp_ns: u64,
    ) -> Self {
        lease.mark_ready();
        let (width, height) = lease.size();
        Self {
            inner: Arc::new(MacGpuSurfaceInner {
                lease,
                _textures: textures,
            }),
            width,
            height,
            timestamp_ns,
        }
    }

    pub fn pixel_buffer(&self) -> &CVPixelBuffer {
        self.inner.lease.pixel_buffer()
    }

    pub const fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    pub const fn timestamp_ns(&self) -> u64 {
        self.timestamp_ns
    }

    pub fn with_timestamp(&self, timestamp_ns: u64) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
            width: self.width,
            height: self.height,
            timestamp_ns,
        }
    }
}
