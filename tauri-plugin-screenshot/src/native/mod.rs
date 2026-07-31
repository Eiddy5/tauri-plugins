mod desktop;
mod magnifier;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

use std::sync::Arc;

use async_trait::async_trait;
use tauri::{AppHandle, Runtime};

use crate::{models::CaptureOptions, CaptureRegion, Result};

pub(crate) struct NativeCaptureImage {
    pub png: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub region: CaptureRegion,
}

pub(crate) enum NativeCaptureOutcome {
    Captured(NativeCaptureImage),
    Cancelled,
}

#[async_trait]
pub(crate) trait NativeCaptureAdapter: Send + Sync {
    async fn capture_area(&self, options: CaptureOptions) -> Result<NativeCaptureOutcome>;
}

pub(crate) fn default_adapter<R: Runtime>(app: &AppHandle<R>) -> Arc<dyn NativeCaptureAdapter> {
    #[cfg(target_os = "macos")]
    {
        Arc::new(macos::MacOsNativeCaptureAdapter::new(app.clone()))
    }

    #[cfg(target_os = "windows")]
    {
        let _ = app;
        Arc::new(windows::WindowsNativeCaptureAdapter)
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = app;
        Arc::new(UnsupportedNativeCaptureAdapter)
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
struct UnsupportedNativeCaptureAdapter;

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
#[async_trait]
impl NativeCaptureAdapter for UnsupportedNativeCaptureAdapter {
    async fn capture_area(&self, _options: CaptureOptions) -> Result<NativeCaptureOutcome> {
        Err(crate::Error::new(
            crate::ErrorCode::Unsupported,
            "当前平台尚不支持原生区域截图",
            false,
        ))
    }
}
