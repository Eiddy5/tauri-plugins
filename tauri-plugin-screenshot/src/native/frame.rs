use std::sync::Arc;

use image::{codecs::png::PngEncoder, ExtendedColorType, ImageEncoder};

use crate::{
    error::{Error, ErrorCode},
    models::CaptureRegion,
    selection::Rect,
    Result,
};

use super::NativeCaptureImage;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PixelFormat {
    #[cfg(any(target_os = "macos", test))]
    Rgba8,
    #[cfg(any(target_os = "windows", test))]
    Bgra8,
}

#[derive(Clone, Debug)]
pub(crate) struct CapturedFrame {
    pixels: Arc<Vec<u8>>,
    pixel_width: u32,
    pixel_height: u32,
    logical_width: f64,
    logical_height: f64,
    format: PixelFormat,
}

impl CapturedFrame {
    pub(crate) fn new(
        pixels: Vec<u8>,
        pixel_width: u32,
        pixel_height: u32,
        logical_width: f64,
        logical_height: f64,
        format: PixelFormat,
    ) -> Result<Self> {
        let expected_len = usize::try_from(pixel_width)
            .ok()
            .and_then(|width| {
                usize::try_from(pixel_height)
                    .ok()
                    .and_then(|height| width.checked_mul(height))
            })
            .and_then(|pixels| pixels.checked_mul(4))
            .ok_or_else(|| Error::new(ErrorCode::CaptureFailed, "截图尺寸超出可处理范围", false))?;
        if pixels.len() != expected_len
            || pixel_width == 0
            || pixel_height == 0
            || !logical_width.is_finite()
            || !logical_height.is_finite()
            || logical_width <= 0.0
            || logical_height <= 0.0
        {
            return Err(Error::new(
                ErrorCode::CaptureFailed,
                "截图帧尺寸或像素数据无效",
                true,
            ));
        }
        Ok(Self {
            pixels: Arc::new(pixels),
            pixel_width,
            pixel_height,
            logical_width,
            logical_height,
            format,
        })
    }

    #[cfg(target_os = "windows")]
    pub(crate) fn pixel_width(&self) -> u32 {
        self.pixel_width
    }

    #[cfg(target_os = "windows")]
    pub(crate) fn pixel_height(&self) -> u32 {
        self.pixel_height
    }

    #[cfg(target_os = "windows")]
    pub(crate) fn pixels(&self) -> Arc<Vec<u8>> {
        Arc::clone(&self.pixels)
    }

    pub(crate) fn crop_png(&self, logical_region: Rect) -> Result<NativeCaptureImage> {
        let region = self.physical_region(logical_region)?;
        let row_len = usize::try_from(region.width)
            .ok()
            .and_then(|width| width.checked_mul(4))
            .ok_or_else(|| invalid_selection("选区宽度超出可处理范围"))?;
        let capacity = row_len
            .checked_mul(region.height as usize)
            .ok_or_else(|| invalid_selection("选区尺寸超出可处理范围"))?;
        let source_stride = self.pixel_width as usize * 4;
        let mut rgba = Vec::with_capacity(capacity);

        for y in region.y..region.y + region.height {
            let start = y as usize * source_stride + region.x as usize * 4;
            let end = start + row_len;
            let row = self
                .pixels
                .get(start..end)
                .ok_or_else(|| invalid_selection("选区超出截图帧范围"))?;
            match self.format {
                #[cfg(any(target_os = "macos", test))]
                PixelFormat::Rgba8 => rgba.extend_from_slice(row),
                #[cfg(any(target_os = "windows", test))]
                PixelFormat::Bgra8 => {
                    for pixel in row.chunks_exact(4) {
                        rgba.extend_from_slice(&[pixel[2], pixel[1], pixel[0], pixel[3]]);
                    }
                }
            }
        }

        Ok(NativeCaptureImage {
            png: encode_png(&rgba, region.width, region.height)?,
            width: region.width,
            height: region.height,
            region,
        })
    }

    fn physical_region(&self, logical_region: Rect) -> Result<CaptureRegion> {
        if !logical_region.x.is_finite()
            || !logical_region.y.is_finite()
            || !logical_region.width.is_finite()
            || !logical_region.height.is_finite()
            || logical_region.width <= 0.0
            || logical_region.height <= 0.0
        {
            return Err(invalid_selection("选区无效"));
        }

        let scale_x = f64::from(self.pixel_width) / self.logical_width;
        let scale_y = f64::from(self.pixel_height) / self.logical_height;
        let left = (logical_region.x * scale_x)
            .floor()
            .clamp(0.0, f64::from(self.pixel_width)) as u32;
        let top = (logical_region.y * scale_y)
            .floor()
            .clamp(0.0, f64::from(self.pixel_height)) as u32;
        let right = (logical_region.right() * scale_x)
            .ceil()
            .clamp(0.0, f64::from(self.pixel_width)) as u32;
        let bottom = (logical_region.bottom() * scale_y)
            .ceil()
            .clamp(0.0, f64::from(self.pixel_height)) as u32;
        let width = right.saturating_sub(left);
        let height = bottom.saturating_sub(top);
        if width == 0 || height == 0 {
            return Err(invalid_selection("选区映射到物理像素后为空"));
        }
        Ok(CaptureRegion {
            x: left,
            y: top,
            width,
            height,
        })
    }
}

fn encode_png(rgba: &[u8], width: u32, height: u32) -> Result<Vec<u8>> {
    let mut png = Vec::new();
    PngEncoder::new(&mut png)
        .write_image(rgba, width, height, ExtendedColorType::Rgba8)
        .map_err(|error| {
            Error::new(
                ErrorCode::CaptureFailed,
                format!("PNG 编码失败：{error}"),
                true,
            )
        })?;
    Ok(png)
}

fn invalid_selection(message: impl Into<String>) -> Error {
    Error::new(ErrorCode::InvalidSelection, message, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crop_maps_logical_retina_coordinates_outward() {
        let pixels = vec![255; 8 * 8 * 4];
        let frame = CapturedFrame::new(pixels, 8, 8, 4.0, 4.0, PixelFormat::Rgba8).expect("frame");
        let region = frame
            .physical_region(Rect {
                x: 0.25,
                y: 0.25,
                width: 1.5,
                height: 1.5,
            })
            .expect("region");

        assert_eq!(
            region,
            CaptureRegion {
                x: 0,
                y: 0,
                width: 4,
                height: 4,
            }
        );
    }

    #[test]
    fn bgra_crop_is_encoded_as_rgba_png() {
        let frame = CapturedFrame::new(vec![10, 20, 30, 255], 1, 1, 1.0, 1.0, PixelFormat::Bgra8)
            .expect("frame");
        let result = frame
            .crop_png(Rect {
                x: 0.0,
                y: 0.0,
                width: 1.0,
                height: 1.0,
            })
            .expect("crop");
        let decoded = image::load_from_memory(&result.png)
            .expect("decode")
            .into_rgba8();

        assert_eq!(decoded.get_pixel(0, 0).0, [30, 20, 10, 255]);
    }
}
