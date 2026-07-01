use std::marker::PhantomData;

use serde::de::DeserializeOwned;
use tauri::{plugin::PluginApi, AppHandle, Runtime};

use crate::{NetWatcherConfig, NetWatcherSnapshot, Result, StartWatchingOptions};

#[cfg(target_os = "ios")]
tauri::ios_plugin_binding!(init_plugin_net_watcher);

// Mobile watcher support is intentionally not implemented yet.
pub fn init<R, C>(
    _app: &AppHandle<R>,
    _api: PluginApi<R, C>,
    config: NetWatcherConfig,
) -> Result<NetWatcher<R>>
where
    R: Runtime,
    C: DeserializeOwned,
{
    if config.auto_start {
        return Err(crate::Error::unsupported_platform());
    }

    Ok(NetWatcher {
        config,
        _runtime: PhantomData,
    })
}

/// Access to the net-watcher APIs.
pub struct NetWatcher<R: Runtime> {
    config: NetWatcherConfig,
    _runtime: PhantomData<R>,
}

impl<R: Runtime> NetWatcher<R> {
    pub(crate) async fn get_snapshot(&self) -> Result<NetWatcherSnapshot> {
        Err(crate::Error::unsupported_platform())
    }

    pub(crate) async fn start_watching(
        &self,
        _options: Option<StartWatchingOptions>,
    ) -> Result<()> {
        Err(crate::Error::unsupported_platform())
    }

    pub(crate) async fn stop_watching(&self) -> Result<()> {
        Err(crate::Error::unsupported_platform())
    }

    pub(crate) async fn get_config(&self) -> Result<NetWatcherConfig> {
        Ok(self.config.clone())
    }
}
