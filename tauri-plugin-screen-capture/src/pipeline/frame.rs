use std::sync::Arc;

use crate::models::PixelFormat;

#[derive(Debug, Clone)]
pub struct VideoFrame {
    pub width: u32,
    pub height: u32,
    pub pixel_format: PixelFormat,
    pub timestamp_ns: u64,
    pub data: Arc<[u8]>,
}
