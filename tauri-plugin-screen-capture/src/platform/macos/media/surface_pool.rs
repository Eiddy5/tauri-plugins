use std::sync::{Arc, Mutex};

use apple_cf::{cv::CVPixelBuffer, iosurface::IOSurface};
use thiserror::Error;

use super::MacMetalDevice;

const BGRA: u32 = u32::from_be_bytes(*b"BGRA");
const BGRA_BYTES_PER_PIXEL: usize = 4;
const METAL_BYTES_PER_ROW_ALIGNMENT: usize = 16;

pub(crate) fn create_aligned_bgra_surface(width: u32, height: u32) -> Option<IOSurface> {
    let tight_bytes_per_row = (width as usize).checked_mul(BGRA_BYTES_PER_PIXEL)?;
    let bytes_per_row = tight_bytes_per_row.checked_add(METAL_BYTES_PER_ROW_ALIGNMENT - 1)?
        & !(METAL_BYTES_PER_ROW_ALIGNMENT - 1);
    let alloc_size = bytes_per_row.checked_mul(height as usize)?;
    IOSurface::create_with_properties(
        width as usize,
        height as usize,
        BGRA,
        BGRA_BYTES_PER_PIXEL,
        bytes_per_row,
        alloc_size,
        None,
    )
}

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum SurfacePoolError {
    #[error("surface pool dimensions and capacity must be positive")]
    InvalidConfiguration,
    #[error("failed to allocate IOSurface-backed pixel buffer")]
    AllocationFailed,
    #[error("surface pool is exhausted")]
    Backpressure,
    #[error("pixel buffer is not IOSurface-backed")]
    MissingIoSurface,
    #[error("unsupported pixel format {0:#010x}")]
    UnsupportedPixelFormat(u32),
    #[error("failed to create Metal texture")]
    MetalTextureCreationFailed,
    #[error("surface pool state lock was poisoned")]
    Poisoned,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SlotState {
    Free,
    InUse,
    Ready,
}

struct Slot {
    pixel_buffer: CVPixelBuffer,
    state: SlotState,
}

struct PoolInner {
    width: u32,
    height: u32,
    slots: Mutex<Vec<Slot>>,
}

#[derive(Clone)]
pub struct MacSurfacePool {
    inner: Arc<PoolInner>,
}

impl MacSurfacePool {
    pub fn new(
        device: &MacMetalDevice,
        width: u32,
        height: u32,
        capacity: usize,
    ) -> Result<Self, SurfacePoolError> {
        if width == 0 || height == 0 || capacity == 0 {
            return Err(SurfacePoolError::InvalidConfiguration);
        }

        let mut slots = Vec::with_capacity(capacity);
        for _ in 0..capacity {
            let io_surface = create_aligned_bgra_surface(width, height)
                .ok_or(SurfacePoolError::AllocationFailed)?;
            let pixel_buffer = CVPixelBuffer::create_with_io_surface(&io_surface)
                .map_err(|_| SurfacePoolError::AllocationFailed)?;
            device.texture_for_bgra(&pixel_buffer)?;
            slots.push(Slot {
                pixel_buffer,
                state: SlotState::Free,
            });
        }

        Ok(Self {
            inner: Arc::new(PoolInner {
                width,
                height,
                slots: Mutex::new(slots),
            }),
        })
    }

    pub fn acquire(&self) -> Result<SurfaceLease, SurfacePoolError> {
        let mut slots = self
            .inner
            .slots
            .lock()
            .map_err(|_| SurfacePoolError::Poisoned)?;
        let (index, slot) = slots
            .iter_mut()
            .enumerate()
            .find(|(_, slot)| slot.state == SlotState::Free)
            .ok_or(SurfacePoolError::Backpressure)?;
        slot.state = SlotState::InUse;
        Ok(SurfaceLease {
            pool: Arc::clone(&self.inner),
            index,
            pixel_buffer: slot.pixel_buffer.clone(),
        })
    }

    pub fn capacity(&self) -> usize {
        self.inner
            .slots
            .lock()
            .map(|slots| slots.len())
            .unwrap_or(0)
    }

    pub fn available(&self) -> usize {
        self.inner
            .slots
            .lock()
            .map(|slots| {
                slots
                    .iter()
                    .filter(|slot| slot.state == SlotState::Free)
                    .count()
            })
            .unwrap_or(0)
    }
}

pub struct SurfaceLease {
    pool: Arc<PoolInner>,
    index: usize,
    pixel_buffer: CVPixelBuffer,
}

impl SurfaceLease {
    pub fn pixel_buffer(&self) -> &CVPixelBuffer {
        &self.pixel_buffer
    }

    pub fn size(&self) -> (u32, u32) {
        (self.pool.width, self.pool.height)
    }

    pub(crate) fn mark_ready(&mut self) {
        if let Ok(mut slots) = self.pool.slots.lock() {
            if let Some(slot) = slots.get_mut(self.index) {
                slot.state = SlotState::Ready;
            }
        }
    }
}

impl Drop for SurfaceLease {
    fn drop(&mut self) {
        if let Ok(mut slots) = self.pool.slots.lock() {
            if let Some(slot) = slots.get_mut(self.index) {
                slot.state = SlotState::Free;
            }
        }
    }
}
