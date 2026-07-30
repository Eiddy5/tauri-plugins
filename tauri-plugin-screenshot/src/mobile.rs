use std::marker::PhantomData;

use serde::de::DeserializeOwned;
use tauri::{ipc::Response, plugin::PluginApi, AppHandle, Runtime};

use crate::{
    error::{Error, ErrorCode},
    models::{CaptureOptions, CaptureResponse},
    Result,
};

pub fn init<R: Runtime, C: DeserializeOwned>(
    _app: &AppHandle<R>,
    _api: PluginApi<R, C>,
) -> Result<Screenshot<R>> {
    Ok(Screenshot(PhantomData))
}

/// Mobile placeholder. Native area capture currently targets desktop only.
pub struct Screenshot<R: Runtime>(PhantomData<R>);

impl<R: Runtime> Screenshot<R> {
    pub async fn capture_area(&self, _options: CaptureOptions) -> Result<CaptureResponse> {
        Err(unsupported())
    }

    pub fn take_capture(&self, _capture_id: &str) -> Result<Response> {
        Err(unsupported())
    }
}

fn unsupported() -> Error {
    Error::new(
        ErrorCode::Unsupported,
        "原生区域截图目前仅支持 macOS 和 Windows",
        false,
    )
}
