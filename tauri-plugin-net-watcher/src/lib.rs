use tauri::{
  plugin::{Builder, TauriPlugin},
  Manager, Runtime,
};

pub use models::*;

mod config;
#[cfg(desktop)]
mod desktop;
#[cfg(mobile)]
mod mobile;

mod commands;
mod error;
mod models;
mod stats;

pub use config::{NetWatcherConfig, StartWatchingOptions};
pub use error::{Error, Result};

#[cfg(desktop)]
use desktop::NetWatcher;
#[cfg(mobile)]
use mobile::NetWatcher;

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
pub fn init<R: Runtime>() -> TauriPlugin<R> {
  Builder::new("net-watcher")
    .invoke_handler(tauri::generate_handler![commands::ping])
    .setup(|app, api| {
      #[cfg(mobile)]
      let net_watcher = mobile::init(app, api)?;
      #[cfg(desktop)]
      let net_watcher = desktop::init(app, api)?;
      app.manage(net_watcher);
      Ok(())
    })
    .build()
}
