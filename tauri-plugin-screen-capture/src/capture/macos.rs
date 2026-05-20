use async_trait::async_trait;

use crate::{
    capture::{CaptureBackend, FrameConsumer, RunningCapture},
    models::{CaptureSource, ListSourcesOptions, PermissionStatus, StartCaptureOptions},
    platform::macos::{permissions, screencapturekit},
    Result,
};

#[derive(Debug, Default)]
pub struct MacOsCaptureBackend;

#[async_trait]
impl CaptureBackend for MacOsCaptureBackend {
    async fn check_permission(&self) -> Result<PermissionStatus> {
        Ok(permissions::check_permission())
    }

    async fn request_permission(&self) -> Result<PermissionStatus> {
        Ok(permissions::request_permission())
    }

    async fn list_sources(&self, options: ListSourcesOptions) -> Result<Vec<CaptureSource>> {
        screencapturekit::list_sources(options).await
    }

    async fn start_capture(
        &self,
        options: StartCaptureOptions,
        consumer: Box<dyn FrameConsumer>,
    ) -> Result<Box<dyn RunningCapture>> {
        screencapturekit::start_capture(options, consumer).await
    }
}
