use async_trait::async_trait;

use crate::{
    error::Error,
    models::{CaptureErrorCode, CaptureStats, StartCaptureOptions},
    pipeline::frame::VideoFrame,
    publisher::CapturePublisher,
    Result,
};

#[derive(Debug, Default)]
pub struct AgoraPublisher;

#[async_trait]
impl CapturePublisher for AgoraPublisher {
    async fn start(&self, _options: StartCaptureOptions) -> Result<()> {
        Err(unsupported())
    }

    async fn push_frame(&self, _frame: VideoFrame) -> Result<()> {
        Err(unsupported())
    }

    async fn pause(&self) -> Result<()> {
        Err(unsupported())
    }

    async fn resume(&self) -> Result<()> {
        Err(unsupported())
    }

    async fn stop(&self) -> Result<()> {
        Err(unsupported())
    }

    async fn stats(&self) -> Result<CaptureStats> {
        Err(unsupported())
    }
}

fn unsupported() -> Error {
    Error::new(
        CaptureErrorCode::PublisherUnsupported,
        "Agora publisher is not linked in this build",
        false,
    )
}
