use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};

use serde::de::DeserializeOwned;
use tauri::{ipc::Response, plugin::PluginApi, AppHandle, Runtime};
use uuid::Uuid;

use crate::{
    error::{Error, ErrorCode},
    models::{CaptureOptions, CaptureResponse},
    native::{default_adapter, NativeCaptureAdapter, NativeCaptureOutcome},
    Result,
};

pub fn init<R: Runtime, C: DeserializeOwned>(
    app: &AppHandle<R>,
    _api: PluginApi<R, C>,
) -> Result<Screenshot<R>> {
    Ok(Screenshot {
        _app: app.clone(),
        adapter: default_adapter(app),
        active: AtomicBool::new(false),
        pending_results: Mutex::new(VecDeque::new()),
    })
}

struct StoredCapture {
    id: String,
    png: Vec<u8>,
}

/// Access to the screenshot Interface.
pub struct Screenshot<R: Runtime> {
    _app: AppHandle<R>,
    adapter: Arc<dyn NativeCaptureAdapter>,
    active: AtomicBool,
    pending_results: Mutex<VecDeque<StoredCapture>>,
}

impl<R: Runtime> Screenshot<R> {
    pub async fn capture_area(&self, options: CaptureOptions) -> Result<CaptureResponse> {
        let _active = ActiveCapture::acquire(&self.active)?;

        match self.adapter.capture_area(options).await? {
            NativeCaptureOutcome::Cancelled => Ok(CaptureResponse::Cancelled),
            NativeCaptureOutcome::Captured(image) => {
                let capture_id = Uuid::new_v4().to_string();
                let mut pending = self
                    .pending_results
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                pending.push_back(StoredCapture {
                    id: capture_id.clone(),
                    png: image.png,
                });
                while pending.len() > 4 {
                    pending.pop_front();
                }
                Ok(CaptureResponse::Captured {
                    capture_id,
                    mime_type: "image/png".to_owned(),
                    width: image.width,
                    height: image.height,
                    region: image.region,
                })
            }
        }
    }

    pub fn take_capture(&self, capture_id: &str) -> Result<Response> {
        let mut pending = self
            .pending_results
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let index = pending
            .iter()
            .position(|capture| capture.id == capture_id)
            .ok_or_else(|| {
                Error::new(
                    ErrorCode::ResultUnavailable,
                    "截图结果不存在、已读取或已过期",
                    true,
                )
            })?;
        let capture = pending
            .remove(index)
            .expect("capture index was resolved from the same queue");
        Ok(Response::new(capture.png))
    }
}

struct ActiveCapture<'a>(&'a AtomicBool);

impl<'a> ActiveCapture<'a> {
    fn acquire(active: &'a AtomicBool) -> Result<Self> {
        active
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| Error::new(ErrorCode::Busy, "已有截图会话正在进行", true))?;
        Ok(Self(active))
    }
}

impl Drop for ActiveCapture<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}
