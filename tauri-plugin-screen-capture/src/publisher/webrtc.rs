use async_trait::async_trait;
use tokio::sync::Mutex;

use crate::{
    error::Error,
    models::CaptureErrorCode,
    models::{CaptureStats, StartCaptureOptions},
    pipeline::frame::VideoFrame,
    publisher::CapturePublisher,
    webrtc::{
        h264_encoder::H264Encoder, signaling::WebRtcSignalingState, track::WebRtcH264SampleSender,
    },
    Result,
};

pub struct WebRtcPublisher {
    signaling: std::sync::Arc<WebRtcSignalingState>,
    h264_sender: WebRtcH264SampleSender,
    encoder: Mutex<Option<H264Encoder>>,
    state: Mutex<WebRtcPublisherState>,
}

#[derive(Debug, Default)]
struct WebRtcPublisherState {
    lifecycle: WebRtcPublisherLifecycle,
    stats: CaptureStats,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum WebRtcPublisherLifecycle {
    #[default]
    Idle,
    Started,
    Paused,
    Stopped,
}

impl WebRtcPublisher {
    pub fn new(signaling: WebRtcSignalingState) -> Self {
        let signaling = std::sync::Arc::new(signaling);
        let h264_sender = WebRtcH264SampleSender::new(signaling.video_track());
        Self {
            signaling,
            h264_sender,
            encoder: Mutex::new(None),
            state: Mutex::new(WebRtcPublisherState::default()),
        }
    }

    pub fn signaling(&self) -> std::sync::Arc<WebRtcSignalingState> {
        std::sync::Arc::clone(&self.signaling)
    }
}

#[async_trait]
impl CapturePublisher for WebRtcPublisher {
    async fn start(&self, options: StartCaptureOptions) -> Result<()> {
        let width = options.width.unwrap_or(1280);
        let height = options.height.unwrap_or(720);
        let fps = options.effective_fps();
        *self.encoder.lock().await = Some(H264Encoder::new(width, height, fps)?);

        let mut state = self.state.lock().await;
        state.lifecycle = WebRtcPublisherLifecycle::Started;
        state.stats = CaptureStats {
            started: true,
            ..CaptureStats::default()
        };
        Ok(())
    }

    async fn push_frame(&self, frame: VideoFrame) -> Result<()> {
        {
            let mut state = self.state.lock().await;
            if state.lifecycle != WebRtcPublisherLifecycle::Started {
                state.stats.frames_dropped += 1;
                return Ok(());
            }
        }

        {
            let mut encoder = self.encoder.lock().await;
            let encoder = encoder.as_mut().ok_or_else(|| {
                Error::new(
                    CaptureErrorCode::WebRtcTrackFailed,
                    "H264 encoder is not initialized",
                    true,
                )
            })?;
            if let Some(sample) = encoder.encode_frame(&frame)? {
                self.h264_sender.send_sample(sample).await?;
                let mut state = self.state.lock().await;
                state.stats.frames_published += 1;
            } else {
                let mut state = self.state.lock().await;
                state.stats.frames_dropped += 1;
            }
        }
        Ok(())
    }

    async fn pause(&self) -> Result<()> {
        let mut state = self.state.lock().await;
        state.lifecycle = WebRtcPublisherLifecycle::Paused;
        state.stats.started = false;
        Ok(())
    }

    async fn resume(&self) -> Result<()> {
        let mut state = self.state.lock().await;
        state.lifecycle = WebRtcPublisherLifecycle::Started;
        state.stats.started = true;
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        {
            let mut state = self.state.lock().await;
            state.lifecycle = WebRtcPublisherLifecycle::Stopped;
            state.stats.started = false;
        }
        *self.encoder.lock().await = None;
        self.signaling.close().await
    }

    async fn stats(&self) -> Result<CaptureStats> {
        Ok(self.state.lock().await.stats.clone())
    }
}
