use async_trait::async_trait;

use crate::{
    capture::{CaptureBackend, FrameConsumer, RunningCapture},
    error::Error,
    models::{
        CaptureErrorCode, CaptureSource, CaptureSourceKind, ListSourcesOptions, PermissionStatus,
        StartCaptureOptions,
    },
    Result,
};

#[derive(Debug, Default)]
pub struct DummyCaptureBackend;

#[async_trait]
impl CaptureBackend for DummyCaptureBackend {
    async fn check_permission(&self) -> Result<PermissionStatus> {
        Ok(PermissionStatus::Granted)
    }

    async fn request_permission(&self) -> Result<PermissionStatus> {
        Ok(PermissionStatus::Granted)
    }

    async fn list_sources(&self, options: ListSourcesOptions) -> Result<Vec<CaptureSource>> {
        let mut sources = Vec::new();
        if options.kinds.contains(&CaptureSourceKind::Display) {
            sources.push(CaptureSource {
                id: "dummy-display-1".to_string(),
                kind: CaptureSourceKind::Display,
                name: "Dummy Display".to_string(),
                title: Some("Dummy Display".to_string()),
                app_name: None,
                pid: None,
                width: 1280,
                height: 720,
                scale_factor: 1.0,
                is_primary: true,
                thumbnail_base64: None,
                filtered_reason: None,
            });
        }
        if options.kinds.contains(&CaptureSourceKind::Window) {
            sources.push(CaptureSource {
                id: "dummy-window-1".to_string(),
                kind: CaptureSourceKind::Window,
                name: "Dummy Window".to_string(),
                title: Some("Dummy Window".to_string()),
                app_name: Some("Screen Capture Test Host".to_string()),
                pid: Some(non_current_pid()),
                width: 800,
                height: 600,
                scale_factor: 1.0,
                is_primary: false,
                thumbnail_base64: None,
                filtered_reason: None,
            });
        }
        Ok(sources)
    }

    async fn start_capture(
        &self,
        _options: StartCaptureOptions,
        _consumer: Box<dyn FrameConsumer>,
    ) -> Result<Box<dyn RunningCapture>> {
        Ok(Box::new(DummyRunningCapture))
    }
}

#[derive(Debug)]
struct DummyRunningCapture;

#[async_trait]
impl RunningCapture for DummyRunningCapture {
    async fn pause(&self) -> Result<()> {
        Ok(())
    }

    async fn resume(&self) -> Result<()> {
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        Ok(())
    }
}

pub fn unsupported_backend_error() -> Error {
    Error::new(
        CaptureErrorCode::UnsupportedPlatform,
        "Screen capture is not implemented on this platform",
        false,
    )
}

fn non_current_pid() -> u32 {
    let current_pid = std::process::id();
    if current_pid == u32::MAX {
        current_pid - 1
    } else {
        current_pid + 1
    }
}
