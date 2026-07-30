use tauri::{command, ipc::Response, AppHandle, Runtime, WebviewWindow};

use crate::{CaptureOptions, CaptureResponse, Result, ScreenshotExt};

#[command]
pub(crate) async fn capture_area<R: Runtime>(
    app: AppHandle<R>,
    window: WebviewWindow<R>,
    options: Option<CaptureOptions>,
) -> Result<CaptureResponse> {
    let result = app
        .screenshot()
        .capture_area(options.unwrap_or_default())
        .await;
    let _ = window.set_focus();
    result
}

#[command]
pub(crate) fn take_capture<R: Runtime>(app: AppHandle<R>, capture_id: String) -> Result<Response> {
    app.screenshot().take_capture(&capture_id)
}
