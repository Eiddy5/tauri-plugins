use serde::de::DeserializeOwned;
use tauri::{plugin::PluginApi, AppHandle, Runtime};

pub fn init<R: Runtime, C: DeserializeOwned>(
    app: &AppHandle<R>,
    _api: PluginApi<R, C>,
    mcp_config: Option<crate::McpConfig>,
    runtime_config: crate::RuntimeConfig,
) -> crate::Result<Mcp<R>> {
    let runtime = crate::CapabilityRuntime::new(runtime_config)?;
    let mcp = mcp_config
        .map(|config| crate::McpServer::start(runtime.clone(), config))
        .transpose()?;
    Ok(Mcp {
        _app: app.clone(),
        runtime,
        mcp,
    })
}

/// Access to the MCP APIs.
pub struct Mcp<R: Runtime> {
    _app: AppHandle<R>,
    runtime: crate::CapabilityRuntime,
    mcp: Option<crate::McpServer>,
}

impl<R: Runtime> Mcp<R> {
    pub fn runtime(&self) -> &crate::CapabilityRuntime {
        &self.runtime
    }
    pub fn mcp_address(&self) -> Option<std::net::SocketAddr> {
        self.mcp.as_ref().map(crate::McpServer::address)
    }
    pub(crate) fn shutdown(&self) {
        self.runtime.shutdown();
        if let Some(server) = &self.mcp {
            server.shutdown();
        }
    }
}
