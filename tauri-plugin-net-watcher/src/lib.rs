use tauri::{
    plugin::{Builder, TauriPlugin},
    Manager, Runtime,
};

pub use models::*;

mod config;
#[cfg(any(target_os = "windows", target_os = "macos"))]
mod desktop;
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
mod mobile;

mod commands;
mod error;
mod internet;
mod models;
mod network;
mod quality;
mod schedule;
mod snapshot;
#[cfg(any(target_os = "windows", target_os = "macos"))]
mod watcher;

pub use config::{NetWatcherConfig, ReachabilityTargetConfig, StartWatchingOptions};
pub use error::{Error, Result};

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
use mobile::NetWatcher;
#[cfg(any(target_os = "windows", target_os = "macos"))]
use watcher::NetWatcher;

/// Extensions to [`tauri::App`], [`tauri::AppHandle`] and [`tauri::Window`] to access the net-watcher APIs.
pub trait NetWatcherExt<R: Runtime> {
    fn net_watcher(&self) -> &NetWatcher<R>;
}

impl<R: Runtime, T: Manager<R>> crate::NetWatcherExt<R> for T {
    fn net_watcher(&self) -> &NetWatcher<R> {
        self.state::<NetWatcher<R>>().inner()
    }
}

/// Initializes the plugin.
pub fn init<R: Runtime>() -> TauriPlugin<R, serde_json::Value> {
    Builder::<R, serde_json::Value>::new("net-watcher")
        .invoke_handler(tauri::generate_handler![
            commands::get_snapshot,
            commands::start_watching,
            commands::stop_watching,
            commands::get_config
        ])
        .setup(|app, api| {
            let config = parse_plugin_config(api.config())?;
            #[cfg(not(any(target_os = "windows", target_os = "macos")))]
            let net_watcher = mobile::init(app, api, config)?;
            #[cfg(any(target_os = "windows", target_os = "macos"))]
            let net_watcher = desktop::init(app, api, config)?;
            app.manage(net_watcher);
            Ok(())
        })
        .build()
}

fn parse_plugin_config(value: &serde_json::Value) -> Result<NetWatcherConfig> {
    let config = if value.is_null() {
        NetWatcherConfig::default()
    } else {
        serde_json::from_value(value.clone()).map_err(|error| {
            Error::invalid_config(format!("invalid net watcher plugin config: {error}"))
        })?
    };

    config.validate()?;

    Ok(config)
}
