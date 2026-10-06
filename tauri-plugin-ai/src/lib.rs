use tauri::{
    plugin::{Builder as PluginBuilder, TauriPlugin},
    Manager, Runtime,
};

pub use models::*;

mod desktop;

mod commands;
mod error;
mod mcp;
mod models;
mod registry;
mod runtime;

pub use error::{Error, Result};
pub use mcp::{McpConfig, McpServer};
pub use runtime::{Authorizer, CapabilityRuntime, EventSink, RuntimeConfig};

pub use desktop::Ai;

/// Extensions to [`tauri::App`], [`tauri::AppHandle`] and [`tauri::Window`] to access the ai APIs.
pub trait AiExt<R: Runtime> {
    fn ai(&self) -> &Ai<R>;
}

impl<R: Runtime, T: Manager<R>> crate::AiExt<R> for T {
    fn ai(&self) -> &Ai<R> {
        self.state::<Ai<R>>().inner()
    }
}

/// Initializes the plugin.
pub fn init<R: Runtime>() -> TauriPlugin<R> {
    Builder::new().build()
}

#[derive(Default)]
pub struct Builder {
    mcp: Option<McpConfig>,
    runtime: RuntimeConfig,
}
impl Builder {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn mcp(mut self, config: McpConfig) -> Self {
        self.mcp = Some(config);
        self
    }
    pub fn runtime(mut self, config: RuntimeConfig) -> Self {
        self.runtime = config;
        self
    }
    pub fn build<R: Runtime>(self) -> TauriPlugin<R> {
        PluginBuilder::new("ai")
            .invoke_handler(tauri::generate_handler![
                commands::runtime_connect,
                commands::runtime_ready,
                commands::runtime_invoke,
                commands::runtime_cancel,
                commands::runtime_resolve,
                commands::runtime_disconnect
            ])
            .setup(move |app, api| {
                let ai = desktop::init(app, api, self.mcp, self.runtime)?;
                app.manage(ai);
                Ok(())
            })
            .on_navigation(|webview, _| {
                webview
                    .app_handle()
                    .ai()
                    .runtime()
                    .disconnect_source(webview.label());
                true
            })
            .on_event(|app, event| match event {
                tauri::RunEvent::WindowEvent {
                    label,
                    event: tauri::WindowEvent::Destroyed,
                    ..
                } => app.ai().runtime().disconnect_source(label),
                tauri::RunEvent::Exit => app.ai().shutdown(),
                _ => {}
            })
            .build()
    }
}
