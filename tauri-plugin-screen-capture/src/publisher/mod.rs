mod webrtc;

use async_trait::async_trait;

use crate::{
    models::{CaptureStats, StartCaptureOptions},
    pipeline::frame::VideoFrame,
    Result,
};

pub use webrtc::WebRtcPublisher;

#[async_trait]
pub trait CapturePublisher: Send + Sync {
    async fn start(&self, options: StartCaptureOptions) -> Result<()>;
    async fn push_frame(&self, frame: VideoFrame) -> Result<()>;
    async fn pause(&self) -> Result<()>;
    async fn resume(&self) -> Result<()>;
    async fn stop(&self) -> Result<()>;
    async fn stats(&self) -> Result<CaptureStats>;
}
