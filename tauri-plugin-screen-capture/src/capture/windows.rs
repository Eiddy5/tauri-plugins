use async_trait::async_trait;

use crate::{
    capture::{CaptureBackend, FrameConsumer, RunningCapture},
    models::{CaptureSource, ListSourcesOptions, PermissionStatus, StartCaptureOptions},
    platform::windows::{graphics_capture, permissions},
    Result,
};

#[derive(Debug, Default)]
pub struct WindowsCaptureBackend;

#[async_trait]
impl CaptureBackend for WindowsCaptureBackend {
    async fn check_permission(&self) -> Result<PermissionStatus> {
        Ok(permissions::check_permission())
    }

    async fn request_permission(&self) -> Result<PermissionStatus> {
        permissions::request_permission()
    }

    async fn list_sources(&self, options: ListSourcesOptions) -> Result<Vec<CaptureSource>> {
        graphics_capture::list_sources(options)
    }

    async fn start_capture(
        &self,
        options: StartCaptureOptions,
        consumer: Box<dyn FrameConsumer>,
    ) -> Result<Box<dyn RunningCapture>> {
        graphics_capture::start_capture(options, consumer)
    }
}
