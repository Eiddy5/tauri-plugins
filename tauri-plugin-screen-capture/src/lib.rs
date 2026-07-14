use std::sync::Arc;
use tauri::{
    plugin::{Builder as TauriPluginBuilder, TauriPlugin},
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
pub mod overlay;
pub mod pipeline;
pub mod platform;
pub mod publisher;
pub mod sources;
mod state;
pub mod webrtc;

pub use error::{Error, Result};
use publisher::CapturePublisherFactory;
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
    Builder::new().build()
}

pub struct Builder {
    publisher_factory: Option<Arc<dyn CapturePublisherFactory>>,
}

impl Builder {
    pub fn new() -> Self {
        Self {
            publisher_factory: None,
        }
    }

    pub fn publisher_factory(
        mut self,
        publisher_factory: Arc<dyn CapturePublisherFactory>,
    ) -> Self {
        self.publisher_factory = Some(publisher_factory);
        self
    }

    pub fn build<R: Runtime>(self) -> TauriPlugin<R> {
        let publisher_factory = self.publisher_factory;
        TauriPluginBuilder::new("screen-capture")
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
            .setup(move |app, api| {
                #[cfg(mobile)]
                let screen_capture = mobile::init(app, api)?;
                #[cfg(desktop)]
                let screen_capture = desktop::init(app, api)?;
                app.manage(screen_capture);
                #[cfg(target_os = "macos")]
                let overlay_factory: Arc<dyn overlay::ShareOverlayFactory> =
                    Arc::new(overlay::macos::MacOsShareOverlayFactory::new(app.clone()));
                #[cfg(not(target_os = "macos"))]
                let overlay_factory: Arc<dyn overlay::ShareOverlayFactory> =
                    Arc::new(overlay::DefaultShareOverlayFactory);
                let state = ScreenCaptureState::with_overlay_factory_and_publisher_factory(
                    overlay_factory,
                    publisher_factory.clone(),
                );
                app.manage(state);
                Ok(())
            })
            .build()
    }
}

impl Default for Builder {
    fn default() -> Self {
        Self::new()
    }
}
