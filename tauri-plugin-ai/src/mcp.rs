use crate::{Caller, CapabilityRuntime, Error, Result};
use axum::{
    extract::Request,
    http::{header, HeaderValue, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    Router,
};
use rmcp::{
    model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, Implementation, ListToolsResult,
        PaginatedRequestParams, ServerCapabilities, ServerConfig, Tool, ToolAnnotations,
    },
    service::RequestContext,
    transport::streamable_http_server::{
        session::local::LocalSessionManager, StreamableHttpServerConfig, StreamableHttpService,
    },
    ErrorData, RoleServer, ServerHandler,
};
use std::{
    net::{Ipv4Addr, SocketAddr, TcpListener},
    sync::Arc,
};
use tokio_util::sync::CancellationToken;

/// Explicitly enabled local MCP endpoint. Never serialized into frontend metadata.
pub struct McpConfig {
    port: u16,
    token: String,
}
impl McpConfig {
    pub fn localhost(port: u16, token: impl Into<String>) -> Self {
        Self {
            port,
            token: token.into(),
        }
    }
}

pub struct McpServer {
    address: SocketAddr,
    cancellation: CancellationToken,
}
impl McpServer {
    pub fn start(runtime: CapabilityRuntime, config: McpConfig) -> Result<Self> {
        if config.token.len() < 32
            || !config
                .token
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"-_".contains(&c))
        {
            return Err(Error::new(
                "INVALID_CONFIG",
                "MCP token requires at least 32 ASCII letters, digits, hyphens or underscores",
            ));
        }
        let mut authorization = HeaderValue::from_str(&format!("Bearer {}", config.token))
            .map_err(|_| Error::new("INVALID_CONFIG", "Invalid MCP credential"))?;
        authorization.set_sensitive(true);
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, config.port))?;
        listener.set_nonblocking(true)?;
        let address = listener.local_addr()?;
        let cancellation = CancellationToken::new();
        let http_config = StreamableHttpServerConfig::default()
            .enforce_origin_validation()
            .with_max_request_body_bytes(crate::registry::MAX_BYTES)
            .with_cancellation_token(cancellation.clone());
        let service = StreamableHttpService::new(
            move || Ok(McpAdapter(runtime.clone())),
            Arc::new(LocalSessionManager::default()),
            http_config,
        );
        let router = Router::new()
            .route_service("/mcp", service)
            .layer(middleware::from_fn(move |request: Request, next: Next| {
                let authorization = authorization.clone();
                async move { authenticate(request, next, authorization).await }
            }));
        let shutdown = cancellation.clone();
        tauri::async_runtime::spawn(async move {
            match tokio::net::TcpListener::from_std(listener) {
                Ok(listener) => {
                    if let Err(error) = axum::serve(listener, router)
                        .with_graceful_shutdown(shutdown.cancelled_owned())
                        .await
                    {
                        eprintln!("AI MCP server stopped: {error}");
                    }
                }
                Err(error) => eprintln!("AI MCP listener failed: {error}"),
            }
        });
        Ok(Self {
            address,
            cancellation,
        })
    }

    pub fn address(&self) -> SocketAddr {
        self.address
    }
    pub fn shutdown(&self) {
        self.cancellation.cancel();
    }
}
impl Drop for McpServer {
    fn drop(&mut self) {
        self.shutdown();
    }
}

async fn authenticate(request: Request, next: Next, expected: HeaderValue) -> Response {
    if request.headers().get(header::AUTHORIZATION) != Some(&expected) {
        return (
            StatusCode::UNAUTHORIZED,
            [(header::WWW_AUTHENTICATE, "Bearer realm=\"tauri-ai\"")],
        )
            .into_response();
    }
    next.run(request).await
}

#[derive(Clone)]
struct McpAdapter(CapabilityRuntime);
impl ServerHandler for McpAdapter {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build()).with_server_info(
            Implementation::new("tauri-plugin-ai", env!("CARGO_PKG_VERSION")),
        )
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> std::result::Result<ListToolsResult, ErrorData> {
        let tools = self
            .0
            .list_tools(&Caller::mcp())
            .into_iter()
            .map(|definition| {
                let mut tool = Tool::new(
                    definition.name,
                    definition.description,
                    definition.input_schema.as_object().unwrap().clone(),
                );
                tool.output_schema = Some(Arc::new(
                    definition.output_schema.as_object().unwrap().clone(),
                ));
                let mut annotations = ToolAnnotations::new();
                annotations.read_only_hint = definition.annotations.read_only_hint;
                annotations.destructive_hint = definition.annotations.destructive_hint;
                annotations.idempotent_hint = definition.annotations.idempotent_hint;
                annotations.open_world_hint = definition.annotations.open_world_hint;
                tool.annotations = Some(annotations);
                tool
            })
            .collect();
        Ok(ListToolsResult::with_all_items(tools))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> std::result::Result<CallToolResponse, ErrorData> {
        let arguments = serde_json::Value::Object(request.arguments.unwrap_or_default());
        let result = match self
            .0
            .call(Caller::mcp(), &request.name, arguments, context.ct)
            .await
        {
            Ok(value) => CallToolResult::structured(value),
            Err(error) if error.protocol_error => {
                return Err(ErrorData::invalid_params(
                    "Unknown tool",
                    Some(serde_json::to_value(error).expect("error is JSON")),
                ));
            }
            Err(error) => CallToolResult::structured_error(
                serde_json::to_value(error).expect("error is JSON"),
            ),
        };
        Ok(result.into())
    }
}
