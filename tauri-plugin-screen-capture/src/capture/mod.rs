mod dummy;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

use async_trait::async_trait;
use tokio::sync::watch;

use crate::{
    models::{
        CaptureErrorCode, CaptureErrorPayload, CaptureSource, CaptureSourceKind, CaptureStats,
        ListSourcesOptions, PermissionStatus, StartCaptureOptions,
    },
    pipeline::frame::VideoFrame,
    Result,
};

#[derive(Clone, Debug, PartialEq)]
pub enum CaptureFinishReason {
    SourceClosed { reason: String },
    Error(CaptureErrorPayload),
}

impl CaptureFinishReason {
    pub fn source_closed(reason: impl Into<String>) -> Self {
        Self::SourceClosed {
            reason: reason.into(),
        }
    }

    pub fn error(error: CaptureErrorPayload) -> Self {
        Self::Error(error)
    }

    pub fn into_error_payload(self, source_kind: CaptureSourceKind) -> CaptureErrorPayload {
        match self {
            Self::SourceClosed { reason } => {
                let message = match source_kind {
                    CaptureSourceKind::Window => "被共享窗口已关闭",
                    CaptureSourceKind::Display => "被共享屏幕已断开",
                };
                CaptureErrorPayload {
                    code: CaptureErrorCode::SourceUnavailable,
                    message: message.to_string(),
                    recoverable: true,
                    details: Some(serde_json::json!({ "reason": reason })),
                }
            }
            Self::Error(error) => error,
        }
    }
}

pub use dummy::{unsupported_backend_error, DummyCaptureBackend};
#[cfg(target_os = "macos")]
pub use macos::MacOsCaptureBackend;
#[cfg(target_os = "windows")]
pub use windows::WindowsCaptureBackend;

#[async_trait]
pub trait FrameConsumer: Send + Sync {
    fn supports_gpu_surfaces(&self) -> bool {
        false
    }

    async fn push_frame(&self, frame: VideoFrame) -> Result<()>;

    #[cfg(target_os = "windows")]
    async fn push_gpu_surface(
        &self,
        _surface: crate::platform::windows::media::WindowsGpuSurface,
    ) -> Result<()> {
        Err(crate::Error::new(
            crate::CaptureErrorCode::PublisherUnsupported,
            "frame consumer does not support Windows GPU surfaces",
            true,
        ))
    }
}

#[async_trait]
pub trait CaptureBackend: Send + Sync {
    async fn check_permission(&self) -> Result<PermissionStatus>;
    async fn request_permission(&self) -> Result<PermissionStatus>;
    async fn list_sources(&self, options: ListSourcesOptions) -> Result<Vec<CaptureSource>>;
    async fn start_capture(
        &self,
        options: StartCaptureOptions,
        consumer: Box<dyn FrameConsumer>,
    ) -> Result<Box<dyn RunningCapture>>;
}

#[async_trait]
pub trait RunningCapture: Send + Sync {
    async fn pause(&self) -> Result<()>;
    async fn resume(&self) -> Result<()>;
    async fn stop(&self) -> Result<()>;

    fn finish_receiver(&self) -> Option<watch::Receiver<Option<CaptureFinishReason>>> {
        None
    }

    async fn stats(&self) -> Result<CaptureStats> {
        Ok(CaptureStats::default())
    }
}
