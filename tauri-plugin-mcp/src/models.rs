use serde::{Deserialize, Serialize};

pub const BRIDGE_PROTOCOL: u32 = 2;

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ToolAnnotations {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub read_only_hint: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub destructive_hint: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub idempotent_hint: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub open_world_hint: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ToolPolicy {
    #[serde(default)]
    pub permissions: Vec<String>,
    #[serde(default = "default_timeout")]
    pub timeout_ms: u64,
    #[serde(default = "default_concurrency")]
    pub max_concurrency: usize,
}
fn default_timeout() -> u64 {
    30_000
}
fn default_concurrency() -> usize {
    64
}
impl Default for ToolPolicy {
    fn default() -> Self {
        Self {
            permissions: Vec::new(),
            timeout_ms: default_timeout(),
            max_concurrency: default_concurrency(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
    pub output_schema: serde_json::Value,
    #[serde(default)]
    pub annotations: ToolAnnotations,
    #[serde(default)]
    pub policy: ToolPolicy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum CallSource {
    Local,
    Mcp,
}

/// Constructed by trusted Rust ingress, never deserialized from JS arguments.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Caller {
    pub principal: String,
    pub source: CallSource,
}
impl Caller {
    pub fn local(host: &str) -> Self {
        Self {
            principal: format!("app:{host}"),
            source: CallSource::Local,
        }
    }
    pub fn mcp() -> Self {
        Self {
            principal: "mcp:local".into(),
            source: CallSource::Mcp,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeEvent {
    pub request_id: String,
    pub name: String,
    pub source: CallSource,
    pub caller: Caller,
    pub state: String,
    pub duration_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum BridgeEvent {
    Call {
        session_id: String,
        request_id: String,
        name: String,
        arguments: serde_json::Value,
        deadline: u64,
        caller: Caller,
    },
    Cancel {
        session_id: String,
        request_id: String,
        error: crate::Error,
    },
    State {
        session_id: String,
        event: RuntimeEvent,
        pending_count: usize,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "camelCase", deny_unknown_fields)]
pub enum Completion {
    Success { result: serde_json::Value },
    Error { error: crate::Error },
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BridgeSession {
    pub session_id: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeSnapshot {
    pub tools: Vec<ToolDefinition>,
    pub pending_count: usize,
}
