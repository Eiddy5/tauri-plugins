use async_trait::async_trait;
use tokio::sync::Mutex;

use crate::{
    error::Error,
    models::{CaptureErrorCode, CaptureStats, StartCaptureOptions},
    pipeline::frame::VideoFrame,
    publisher::CapturePublisher,
    Result,
};

#[derive(Debug, Default)]
pub struct NullPublisher {
    state: Mutex<NullPublisherState>,
}

#[derive(Debug, Default)]
struct NullPublisherState {
    lifecycle: PublisherLifecycle,
    stats: CaptureStats,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum PublisherLifecycle {
    #[default]
    Idle,
    Started,
    Paused,
    Stopped,
}

#[async_trait]
impl CapturePublisher for NullPublisher {
    async fn start(&self, _options: StartCaptureOptions) -> Result<()> {
        let mut state = self.state.lock().await;
        state.lifecycle = PublisherLifecycle::Started;
        state.stats = CaptureStats {
            started: true,
            ..CaptureStats::default()
        };
        Ok(())
    }

    async fn push_frame(&self, _frame: VideoFrame) -> Result<()> {
        let mut state = self.state.lock().await;
        if state.lifecycle != PublisherLifecycle::Started {
            return Err(Error::new(
                CaptureErrorCode::CaptureRuntimeFailed,
                "publisher is not started",
                true,
            ));
        }
        state.stats.frames_published += 1;
        Ok(())
    }

    async fn pause(&self) -> Result<()> {
        let mut state = self.state.lock().await;
        if state.lifecycle != PublisherLifecycle::Started {
            return Err(invalid_lifecycle_error());
        }
        state.lifecycle = PublisherLifecycle::Paused;
        state.stats.started = false;
        Ok(())
    }

    async fn resume(&self) -> Result<()> {
        let mut state = self.state.lock().await;
        if state.lifecycle != PublisherLifecycle::Paused {
            return Err(invalid_lifecycle_error());
        }
        state.lifecycle = PublisherLifecycle::Started;
        state.stats.started = true;
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        let mut state = self.state.lock().await;
        state.lifecycle = PublisherLifecycle::Stopped;
        state.stats.started = false;
        Ok(())
    }

    async fn stats(&self) -> Result<CaptureStats> {
        Ok(self.state.lock().await.stats.clone())
    }
}

fn invalid_lifecycle_error() -> Error {
    Error::new(
        CaptureErrorCode::CaptureRuntimeFailed,
        "publisher lifecycle transition is invalid",
        true,
    )
}
