use std::io::Write;
use tauri::Manager;
use tauri_plugin_mcp::{McpConfig, McpExt};

pub fn run() {
    let token =
        std::env::var("MCP_TOKEN").unwrap_or_else(|_| uuid::Uuid::new_v4().simple().to_string());
    let port = std::env::var("MCP_PORT")
        .ok()
        .map(|value| value.parse::<u16>().expect("MCP_PORT must be a valid port"))
        .unwrap_or(38473);
    tauri::Builder::default()
        .plugin(
            tauri_plugin_mcp::Builder::new()
                .mcp(McpConfig::localhost(port, token.clone()))
                .build(),
        )
        .setup(move |app| {
            let path = app.path().app_local_data_dir()?.join("mcp-connection.json");
            std::fs::create_dir_all(path.parent().expect("application data directory"))?;
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create(true).truncate(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(&path)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
            }
            let address = app.mcp().mcp_address().expect("MCP enabled");
            let config = serde_json::json!({
                "url": format!("http://{address}/mcp"),
                "headers": { "Authorization": format!("Bearer {token}") }
            });
            file.write_all(serde_json::to_string_pretty(&config)?.as_bytes())?;
            eprintln!("MCP connection configuration: {}", path.display());
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("failed to run MCP Framework example");
}
