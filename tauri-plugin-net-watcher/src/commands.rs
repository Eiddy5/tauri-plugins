use serde::{Deserialize, Serialize};
use tauri::{command, AppHandle, Runtime};

use crate::NetWatcherExt;
use crate::Result;

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PingRequest {
    pub value: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PingResponse {
    pub value: Option<String>,
}

#[command]
pub(crate) async fn ping<R: Runtime>(
    app: AppHandle<R>,
    payload: PingRequest,
) -> Result<PingResponse> {
    app.net_watcher().ping(payload)
}
