use tauri::{
  plugin::{Builder, TauriPlugin},
  Manager, Runtime,
};

pub use models::*;

#[cfg(desktop)]
mod desktop;
#[cfg(mobile)]
mod mobile;

pub mod capture;
mod commands;
mod error;
mod models;
pub mod pipeline;
pub mod platform;
pub mod publisher;
pub mod sources;
pub mod webrtc;
mod state;

pub use error::{Error, Result};
pub use state::ScreenCaptureState;

#[cfg(desktop)]
use desktop::ScreenCapture;
#[cfg(mobile)]
use mobile::ScreenCapture;

/// Extensions to [`tauri::App`], [`tauri::AppHandle`] and [`tauri::Window`] to access the screen-capture APIs.
pub trait ScreenCaptureExt<R: Runtime> {
  fn screen_capture(&self) -> &ScreenCapture<R>;
}

impl<R: Runtime, T: Manager<R>> crate::ScreenCaptureExt<R> for T {
  fn screen_capture(&self) -> &ScreenCapture<R> {
    self.state::<ScreenCapture<R>>().inner()
  }
}

/// Initializes the plugin.
pub fn init<R: Runtime>() -> TauriPlugin<R> {
  Builder::new("screen-capture")
    .invoke_handler(tauri::generate_handler![
      commands::get_capabilities,
      commands::check_permission,
      commands::request_permission,
      commands::list_sources,
      commands::start_capture,
      commands::pause_capture,
      commands::resume_capture,
      commands::stop_capture,
      commands::get_capture_session,
      commands::get_capture_stats,
      commands::create_webrtc_offer,
      commands::accept_webrtc_answer,
      commands::add_webrtc_ice_candidate
    ])
    .setup(|app, api| {
      #[cfg(mobile)]
      let screen_capture = mobile::init(app, api)?;
      #[cfg(desktop)]
      let screen_capture = desktop::init(app, api)?;
      app.manage(screen_capture);
      app.manage(ScreenCaptureState::default());
      Ok(())
    })
    .build()
}
