use serde::de::DeserializeOwned;
use tauri::{plugin::PluginApi, AppHandle, Runtime};

use crate::{watcher::NetWatcher, NetWatcherConfig, Result};

pub fn init<R, C>(
    app: &AppHandle<R>,
    _api: PluginApi<R, C>,
    config: NetWatcherConfig,
) -> Result<NetWatcher<R>>
where
    R: Runtime,
    C: DeserializeOwned,
{
    let watcher = NetWatcher::new(app.clone(), config.clone());

    if config.auto_start {
        watcher.start_background(None)?;
    }

    Ok(watcher)
}
