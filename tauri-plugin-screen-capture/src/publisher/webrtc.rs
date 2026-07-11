use async_trait::async_trait;
use std::{collections::VecDeque, time::Instant};
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
    pending_frame: Mutex<Option<VideoFrame>>,
    state: Mutex<WebRtcPublisherState>,
}

#[derive(Debug, Default)]
struct WebRtcPublisherState {
    lifecycle: WebRtcPublisherLifecycle,
    stats: CaptureStats,
    last_keyframe_request_seen: u64,
    published_at: VecDeque<Instant>,
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
            pending_frame: Mutex::new(None),
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
        state.last_keyframe_request_seen = self.signaling.pending_keyframe_requests();
        state.published_at.clear();
        drop(state);

        if let Some(frame) = self.pending_frame.lock().await.take() {
            self.push_frame(frame).await?;
        }
        Ok(())
    }

    async fn push_frame(&self, frame: VideoFrame) -> Result<()> {
        {
            let state = self.state.lock().await;
            if state.lifecycle != WebRtcPublisherLifecycle::Started {
                *self.pending_frame.lock().await = Some(frame);
                return Ok(());
            }
        }

        self.sync_keyframe_request().await?;

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
                record_published_frame(&mut state, Instant::now());
                if state.stats.frames_published == 1 || state.stats.frames_published % 30 == 0 {
                    eprintln!(
                        "[screen-capture] WebRTC published frame count={}",
                        state.stats.frames_published
                    );
                }
            } else {
                let mut state = self.state.lock().await;
                state.stats.frames_dropped += 1;
            }
        }
        Ok(())
    }

    async fn request_keyframe(&self) -> Result<()> {
        self.signaling.request_keyframe();
        {
            let mut state = self.state.lock().await;
            state.last_keyframe_request_seen = self.signaling.pending_keyframe_requests();
        }
        if let Some(encoder) = self.encoder.lock().await.as_mut() {
            encoder.force_keyframe()?;
        }
        Ok(())
    }

    async fn pause(&self) -> Result<()> {
        let mut state = self.state.lock().await;
        state.lifecycle = WebRtcPublisherLifecycle::Paused;
        state.stats.started = false;
        state.stats.fps = 0.0;
        state.published_at.clear();
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
        *self.pending_frame.lock().await = None;
        self.signaling.close().await
    }

    async fn stats(&self) -> Result<CaptureStats> {
        let mut state = self.state.lock().await;
        refresh_published_fps(&mut state, Instant::now());
        Ok(state.stats.clone())
    }
}

fn record_published_frame(state: &mut WebRtcPublisherState, now: Instant) {
    state.published_at.push_back(now);
    refresh_published_fps(state, now);
}

fn refresh_published_fps(state: &mut WebRtcPublisherState, now: Instant) {
    while state
        .published_at
        .front()
        .is_some_and(|timestamp| now.duration_since(*timestamp).as_secs_f64() > 1.0)
    {
        state.published_at.pop_front();
    }
    state.stats.fps = state.published_at.len() as f64;
}

impl WebRtcPublisher {
    async fn sync_keyframe_request(&self) -> Result<()> {
        let requested = self.signaling.pending_keyframe_requests();
        let mut state = self.state.lock().await;
        if requested > state.last_keyframe_request_seen {
            state.last_keyframe_request_seen = requested;
            drop(state);
            if let Some(encoder) = self.encoder.lock().await.as_mut() {
                encoder.force_keyframe()?;
            }
        }
        Ok(())
    }
}
