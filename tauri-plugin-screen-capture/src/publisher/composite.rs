use std::sync::Arc;

use async_trait::async_trait;

use crate::{
    models::{CaptureStats, StartCaptureOptions},
    pipeline::frame::VideoFrame,
    publisher::CapturePublisher,
    Result,
};

pub struct CompositePublisher {
    publishers: Vec<Arc<dyn CapturePublisher>>,
}

impl CompositePublisher {
    pub fn new(publishers: Vec<Arc<dyn CapturePublisher>>) -> Self {
        Self { publishers }
    }
}

#[async_trait]
impl CapturePublisher for CompositePublisher {
    async fn start(&self, options: StartCaptureOptions) -> Result<()> {
        for publisher in &self.publishers {
            publisher.start(options.clone()).await?;
        }
        Ok(())
    }

    async fn push_frame(&self, frame: VideoFrame) -> Result<()> {
        for publisher in &self.publishers {
            publisher.push_frame(frame.clone()).await?;
        }
        Ok(())
    }

    async fn pause(&self) -> Result<()> {
        for publisher in &self.publishers {
            publisher.pause().await?;
        }
        Ok(())
    }

    async fn resume(&self) -> Result<()> {
        for publisher in &self.publishers {
            publisher.resume().await?;
        }
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        for publisher in &self.publishers {
            publisher.stop().await?;
        }
        Ok(())
    }

    async fn stats(&self) -> Result<CaptureStats> {
        let mut combined = CaptureStats::default();
        for publisher in &self.publishers {
            let stats = publisher.stats().await?;
            combined.frames_captured += stats.frames_captured;
            combined.frames_published += stats.frames_published;
            combined.frames_dropped += stats.frames_dropped;
            combined.bitrate_kbps += stats.bitrate_kbps;
            combined.fps = combined.fps.max(stats.fps);
            combined.started |= stats.started;
        }
        Ok(combined)
    }
}
