use std::sync::Arc;

use windows::Win32::Graphics::Direct3D11::{
    ID3D11Device, ID3D11Texture2D, ID3D11VideoProcessorOutputView, D3D11_CPU_ACCESS_READ,
    D3D11_MAPPED_SUBRESOURCE, D3D11_MAP_READ, D3D11_USAGE_STAGING,
};

use crate::{CaptureErrorCode, Error, Result};

/// Opaque Windows GPU video frame. D3D11 objects stay private to the Windows
/// media module while the capture pipeline can move the surface without a CPU
/// pixel copy.
#[derive(Clone)]
pub struct WindowsGpuSurface {
    inner: Arc<GpuSurfaceInner>,
    width: u32,
    height: u32,
    timestamp_ns: u64,
}

struct GpuSurfaceInner {
    device: ID3D11Device,
    texture: ID3D11Texture2D,
    output_view: ID3D11VideoProcessorOutputView,
}

// D3D11 resources are free-threaded when the owning device has multithread
// protection enabled. D3D11Nv12Processor establishes that invariant before it
// creates any surface that can cross into the encoder thread.
unsafe impl Send for GpuSurfaceInner {}
unsafe impl Sync for GpuSurfaceInner {}

impl WindowsGpuSurface {
    pub(crate) fn new(
        device: ID3D11Device,
        texture: ID3D11Texture2D,
        output_view: ID3D11VideoProcessorOutputView,
        width: u32,
        height: u32,
    ) -> Self {
        Self {
            inner: Arc::new(GpuSurfaceInner {
                device,
                texture,
                output_view,
            }),
            width,
            height,
            timestamp_ns: 0,
        }
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn timestamp_ns(&self) -> u64 {
        self.timestamp_ns
    }

    pub(crate) fn with_timestamp(&self, timestamp_ns: u64) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
            width: self.width,
            height: self.height,
            timestamp_ns,
        }
    }

    pub(crate) fn is_available(&self) -> bool {
        Arc::strong_count(&self.inner) == 1
    }

    pub(crate) fn device(&self) -> &ID3D11Device {
        &self.inner.device
    }

    pub(crate) fn texture(&self) -> &ID3D11Texture2D {
        &self.inner.texture
    }

    pub(crate) fn output_view(&self) -> &ID3D11VideoProcessorOutputView {
        &self.inner.output_view
    }

    pub(crate) fn readback_nv12(&self) -> Result<Vec<u8>> {
        let mut desc = Default::default();
        unsafe { self.inner.texture.GetDesc(&mut desc) };
        desc.Usage = D3D11_USAGE_STAGING;
        desc.BindFlags = 0;
        desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
        desc.MiscFlags = 0;
        let mut staging = None;
        unsafe {
            self.inner
                .device
                .CreateTexture2D(&desc, None, Some(&mut staging))
        }
        .map_err(surface_error)?;
        let staging =
            staging.ok_or_else(|| surface_error("NV12 staging texture was not created"))?;
        let context = unsafe { self.inner.device.GetImmediateContext() }.map_err(surface_error)?;
        unsafe { context.CopyResource(&staging, &self.inner.texture) };
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        unsafe { context.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped)) }
            .map_err(surface_error)?;

        let row_bytes = self.width as usize;
        let row_count = self.height as usize + self.height as usize / 2;
        let mut data = vec![0u8; row_bytes * row_count];
        for row in 0..row_count {
            let source = unsafe {
                std::slice::from_raw_parts(
                    mapped
                        .pData
                        .cast::<u8>()
                        .add(row * mapped.RowPitch as usize),
                    row_bytes,
                )
            };
            data[row * row_bytes..(row + 1) * row_bytes].copy_from_slice(source);
        }
        unsafe { context.Unmap(&staging, 0) };
        Ok(data)
    }
}

fn surface_error(error: impl std::fmt::Display) -> Error {
    Error::new(
        CaptureErrorCode::WebRtcTrackFailed,
        format!("Windows NV12 GPU surface readback failed: {error}"),
        true,
    )
}
