use tauri::{command, ipc::Channel, AppHandle, Runtime, Webview};

use crate::models::*;
use crate::AiExt;
use crate::Result;

#[command]
pub(crate) fn runtime_connect<R: Runtime>(
    app: AppHandle<R>,
    webview: Webview<R>,
    protocol_version: u32,
    definitions: Vec<ToolDefinition>,
    on_event: Channel<BridgeEvent>,
) -> Result<BridgeSession> {
    if protocol_version != BRIDGE_PROTOCOL {
        return Err(crate::Error::new(
            "PROTOCOL_MISMATCH",
            "Supported bridge protocol: 2",
        ));
    }
    let session_id = app.ai().runtime().connect(
        webview.label().to_owned(),
        definitions,
        std::sync::Arc::new(move |event| {
            on_event
                .send(event)
                .map_err(|_| crate::Error::new("NOT_READY", "JS bridge is unavailable"))
        }),
    )?;
    Ok(BridgeSession { session_id })
}

#[command]
pub(crate) fn runtime_ready<R: Runtime>(
    app: AppHandle<R>,
    webview: Webview<R>,
    session_id: String,
) -> Result<RuntimeSnapshot> {
    app.ai().runtime().ready(webview.label(), &session_id)
}

#[command]
pub(crate) async fn runtime_invoke<R: Runtime>(
    app: AppHandle<R>,
    webview: Webview<R>,
    session_id: String,
    request_id: String,
    name: String,
    arguments: serde_json::Value,
) -> Result<serde_json::Value> {
    app.ai()
        .runtime()
        .invoke(webview.label(), &session_id, request_id, &name, arguments)
        .await
}

#[command]
pub(crate) fn runtime_cancel<R: Runtime>(
    app: AppHandle<R>,
    webview: Webview<R>,
    request_id: String,
) -> Result<()> {
    app.ai()
        .runtime()
        .cancel_owned(&Caller::local(webview.label()), &request_id)
}

#[command]
pub(crate) fn runtime_resolve<R: Runtime>(
    app: AppHandle<R>,
    webview: Webview<R>,
    session_id: String,
    request_id: String,
    completion: Completion,
) -> Result<()> {
    app.ai()
        .runtime()
        .resolve(webview.label(), &session_id, &request_id, completion)
}

#[command]
pub(crate) fn runtime_disconnect<R: Runtime>(
    app: AppHandle<R>,
    webview: Webview<R>,
    session_id: String,
) -> Result<()> {
    app.ai().runtime().disconnect(webview.label(), &session_id)
}
