pub mod frame;
pub mod stats;

use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::Mutex;

use crate::{
    capture::FrameConsumer, models::CaptureStats, pipeline::frame::VideoFrame,
    publisher::CapturePublisher, Result,
};

pub struct CapturePipeline {
    publisher: Arc<dyn CapturePublisher>,
    stats: Mutex<CaptureStats>,
}

impl CapturePipeline {
    pub fn new(publisher: Arc<dyn CapturePublisher>) -> Self {
        Self {
            publisher,
            stats: Mutex::new(CaptureStats::default()),
        }
    }

    pub async fn stats(&self) -> CaptureStats {
        self.stats.lock().await.clone()
    }
}

#[async_trait]
impl FrameConsumer for CapturePipeline {
    async fn push_frame(&self, frame: VideoFrame) -> Result<()> {
        {
            let mut stats = self.stats.lock().await;
            stats.frames_captured += 1;
            stats.started = true;
            if stats.frames_captured == 1 || stats.frames_captured % 30 == 0 {
                eprintln!(
                    "[screen-capture] pipeline captured frame count={}",
                    stats.frames_captured
                );
            }
        }

        self.publisher.push_frame(frame).await?;

        Ok(())
    }
}

#[async_trait]
impl FrameConsumer for Arc<CapturePipeline> {
    async fn push_frame(&self, frame: VideoFrame) -> Result<()> {
        self.as_ref().push_frame(frame).await
    }
}
