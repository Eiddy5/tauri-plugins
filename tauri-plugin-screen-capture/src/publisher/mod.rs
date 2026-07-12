mod agora;
mod composite;
mod webrtc;

use async_trait::async_trait;
use std::sync::Arc;

use crate::{
    models::{CaptureStats, StartCaptureOptions},
    pipeline::frame::VideoFrame,
    webrtc::signaling::WebRtcSignalingState,
    Result,
};

pub use agora::{
    AgoraExternalVideoFrame, AgoraExternalVideoPixelFormat, AgoraPublisher, AgoraVideoSink,
};
pub use composite::CompositePublisher;
pub use webrtc::WebRtcPublisher;

#[async_trait]
pub trait CapturePublisher: Send + Sync {
    fn supports_gpu_surfaces(&self) -> bool {
        false
    }

    async fn start(&self, options: StartCaptureOptions) -> Result<()>;
    async fn push_frame(&self, frame: VideoFrame) -> Result<()>;
    #[cfg(target_os = "windows")]
    async fn push_gpu_surface(
        &self,
        _surface: crate::platform::windows::media::WindowsGpuSurface,
    ) -> Result<()> {
        Err(crate::Error::new(
            crate::CaptureErrorCode::PublisherUnsupported,
            "publisher does not support Windows GPU surfaces",
            true,
        ))
    }
    async fn request_keyframe(&self) -> Result<()> {
        Ok(())
    }
    async fn pause(&self) -> Result<()>;
    async fn resume(&self) -> Result<()>;
    async fn stop(&self) -> Result<()>;
    async fn stats(&self) -> Result<CaptureStats>;
}

pub struct PublisherBundle {
    pub publisher: Arc<dyn CapturePublisher>,
    pub webrtc_signaling: Option<Arc<WebRtcSignalingState>>,
}

impl PublisherBundle {
    pub fn new(
        publisher: Arc<dyn CapturePublisher>,
        webrtc_signaling: Option<Arc<WebRtcSignalingState>>,
    ) -> Self {
        Self {
            publisher,
            webrtc_signaling,
        }
    }

    pub fn without_webrtc<P>(publisher: Arc<P>) -> Self
    where
        P: CapturePublisher + 'static,
    {
        Self {
            publisher,
            webrtc_signaling: None,
        }
    }
}

#[async_trait]
pub trait CapturePublisherFactory: Send + Sync {
    async fn create(&self, options: &StartCaptureOptions) -> Result<PublisherBundle>;
}
