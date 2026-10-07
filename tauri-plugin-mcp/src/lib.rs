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

pub use desktop::Mcp;

/// Extensions to [`tauri::App`], [`tauri::AppHandle`] and [`tauri::Window`] to access the MCP APIs.
pub trait McpExt<R: Runtime> {
    fn mcp(&self) -> &Mcp<R>;
}

impl<R: Runtime, T: Manager<R>> crate::McpExt<R> for T {
    fn mcp(&self) -> &Mcp<R> {
        self.state::<Mcp<R>>().inner()
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
        PluginBuilder::new("mcp")
            .invoke_handler(tauri::generate_handler![
                commands::runtime_connect,
                commands::runtime_ready,
                commands::runtime_invoke,
                commands::runtime_cancel,
                commands::runtime_resolve,
                commands::runtime_disconnect
            ])
            .setup(move |app, api| {
                let mcp = desktop::init(app, api, self.mcp, self.runtime)?;
                app.manage(mcp);
                Ok(())
            })
            .on_navigation(|webview, _| {
                webview
                    .app_handle()
                    .mcp()
                    .runtime()
                    .disconnect_source(webview.label());
                true
            })
            .on_event(|app, event| match event {
                tauri::RunEvent::WindowEvent {
                    label,
                    event: tauri::WindowEvent::Destroyed,
                    ..
                } => app.mcp().runtime().disconnect_source(label),
                tauri::RunEvent::Exit => app.mcp().shutdown(),
                _ => {}
            })
            .build()
    }
}
