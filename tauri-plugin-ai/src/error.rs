use serde::{Deserialize, Serialize};

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, Serialize, Deserialize, thiserror::Error)]
#[error("{code}: {message}")]
pub struct Error {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
    #[serde(skip)]
    pub(crate) protocol_error: bool,
}

impl Error {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            details: None,
            protocol_error: false,
        }
    }

    pub(crate) fn unknown(code: &str, message: &str) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            details: Some(serde_json::json!({ "outcome": "unknown" })),
            protocol_error: false,
        }
    }

    pub(crate) fn missing_tool() -> Self {
        Self {
            protocol_error: true,
            ..Self::new("NOT_FOUND", "Unknown tool")
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        Self::new("IO_ERROR", error.to_string())
    }
}
