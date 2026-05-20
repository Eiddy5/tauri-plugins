use serde::Serialize;
use thiserror::Error;

use crate::models::{CaptureErrorCode, CaptureErrorPayload};

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Error)]
pub enum Error {
    #[error("{payload:?}")]
    Structured { payload: CaptureErrorPayload },
    #[cfg(mobile)]
    #[error(transparent)]
    PluginInvoke(#[from] tauri::plugin::mobile::PluginInvokeError),
}

impl Error {
    pub fn new(code: CaptureErrorCode, message: impl Into<String>, recoverable: bool) -> Self {
        Self::Structured {
            payload: CaptureErrorPayload {
                code,
                message: message.into(),
                recoverable,
                details: None,
            },
        }
    }

    pub fn payload(&self) -> CaptureErrorPayload {
        match self {
            Self::Structured { payload } => payload.clone(),
            #[cfg(mobile)]
            Self::PluginInvoke(error) => CaptureErrorPayload {
                code: CaptureErrorCode::Internal,
                message: error.to_string(),
                recoverable: false,
                details: None,
            },
        }
    }
}

impl Serialize for Error {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.payload().serialize(serializer)
    }
}
