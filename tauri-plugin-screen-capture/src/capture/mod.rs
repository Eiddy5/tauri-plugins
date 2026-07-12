mod dummy;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

use async_trait::async_trait;
use tokio::sync::watch;

use crate::{
    models::{
        CaptureErrorPayload, CaptureSource, CaptureStats, ListSourcesOptions, PermissionStatus,
        StartCaptureOptions,
    },
    pipeline::frame::VideoFrame,
    Result,
};

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

    fn finish_receiver(&self) -> Option<watch::Receiver<Option<CaptureErrorPayload>>> {
        None
    }

    async fn stats(&self) -> Result<CaptureStats> {
        Ok(CaptureStats::default())
    }
}
