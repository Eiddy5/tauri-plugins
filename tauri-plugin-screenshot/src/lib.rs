//! Native area screenshot plugin for Tauri 2.
//!
//! Register the plugin with [`init`], grant `screenshot:default` to the calling
//! window's capability, and use the JavaScript `captureArea()` binding to run
//! the complete capture-and-read flow.
//!
//! ```
//! fn register<R: tauri::Runtime>(builder: tauri::Builder<R>) -> tauri::Builder<R> {
//!     builder.plugin(tauri_plugin_screenshot::init())
//! }
//! ```

use tauri::{
    plugin::{Builder, TauriPlugin},
    Manager, Runtime,
};

pub use models::*;

#[cfg(desktop)]
mod desktop;
#[cfg(mobile)]
mod mobile;

mod commands;
mod error;
mod models;
#[cfg(desktop)]
mod native;
#[cfg(desktop)]
mod selection;

pub use error::{Error, ErrorCode, Result};

#[cfg(desktop)]
use desktop::Screenshot;
#[cfg(mobile)]
use mobile::Screenshot;

/// Extends Tauri managers with access to the plugin's Rust screenshot state.
pub trait ScreenshotExt<R: Runtime> {
    /// Returns the screenshot plugin state registered by [`init`].
    fn screenshot(&self) -> &Screenshot<R>;
}

impl<R: Runtime, T: Manager<R>> crate::ScreenshotExt<R> for T {
    fn screenshot(&self) -> &Screenshot<R> {
        self.state::<Screenshot<R>>().inner()
    }
}

/// Initializes the plugin.
pub fn init<R: Runtime>() -> TauriPlugin<R> {
    Builder::new("screenshot")
        .invoke_handler(tauri::generate_handler![
            commands::capture_area,
            commands::take_capture
        ])
        .setup(|app, api| {
            #[cfg(mobile)]
            let screenshot = mobile::init(app, api)?;
            #[cfg(desktop)]
            let screenshot = desktop::init(app, api)?;
            app.manage(screenshot);
            Ok(())
        })
        .build()
}
