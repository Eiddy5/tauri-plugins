use apple_cf::cv::CVPixelBuffer;
use screencapturekit::metal::{IOSurfaceMetalExt, MetalDevice, MetalTexture};

use super::SurfacePoolError;

pub struct MacMetalDevice {
    inner: MetalDevice,
}

impl MacMetalDevice {
    pub fn system_default() -> Option<Self> {
        MetalDevice::system_default().map(|inner| Self { inner })
    }

    pub fn name(&self) -> String {
        self.inner.name()
    }

    pub fn texture_for_bgra(
        &self,
        pixel_buffer: &CVPixelBuffer,
    ) -> Result<MetalTexture, SurfacePoolError> {
        const BGRA: u32 = u32::from_be_bytes(*b"BGRA");
        if pixel_buffer.pixel_format() != BGRA {
            return Err(SurfacePoolError::UnsupportedPixelFormat(
                pixel_buffer.pixel_format(),
            ));
        }
        let surface = pixel_buffer
            .io_surface()
            .ok_or(SurfacePoolError::MissingIoSurface)?;
        surface
            .create_metal_textures(&self.inner)
            .map(|textures| textures.plane0)
            .ok_or(SurfacePoolError::MetalTextureCreationFailed)
    }

    pub(crate) const fn inner(&self) -> &MetalDevice {
        &self.inner
    }
}
