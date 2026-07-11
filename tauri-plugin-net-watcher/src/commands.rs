use tauri::{command, AppHandle, Runtime};

use crate::{NetWatcherConfig, NetWatcherExt, NetWatcherSnapshot, Result, StartWatchingOptions};

#[command]
pub(crate) async fn get_snapshot<R: Runtime>(app: AppHandle<R>) -> Result<NetWatcherSnapshot> {
    app.net_watcher().get_snapshot().await
}

#[command]
pub(crate) async fn start_watching<R: Runtime>(
    app: AppHandle<R>,
    options: Option<StartWatchingOptions>,
) -> Result<()> {
    app.net_watcher().start_watching(options).await
}

#[command]
pub(crate) async fn stop_watching<R: Runtime>(app: AppHandle<R>) -> Result<()> {
    app.net_watcher().stop_watching().await
}

#[command]
pub(crate) async fn get_config<R: Runtime>(app: AppHandle<R>) -> Result<NetWatcherConfig> {
    app.net_watcher().get_config().await
}
