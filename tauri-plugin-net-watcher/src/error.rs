use serde::{ser::Serializer, Serialize};

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{code}: {message}")]
    Structured { code: &'static str, message: String },
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[cfg(mobile)]
    #[error(transparent)]
    PluginInvoke(#[from] tauri::plugin::mobile::PluginInvokeError),
}

impl Error {
    pub fn unsupported_platform() -> Self {
        Self::Structured {
            code: "unsupported_platform",
            message: "unsupported platform".to_string(),
        }
    }

    pub fn invalid_config(message: impl Into<String>) -> Self {
        Self::Structured {
            code: "invalid_config",
            message: message.into(),
        }
    }

    pub fn already_watching() -> Self {
        Self::Structured {
            code: "already_watching",
            message: "net watcher is already watching".to_string(),
        }
    }

    pub fn not_watching() -> Self {
        Self::Structured {
            code: "not_watching",
            message: "net watcher is not watching".to_string(),
        }
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::Structured {
            code: "internal_error",
            message: message.into(),
        }
    }

    pub fn code(&self) -> &'static str {
        match self {
            Self::Structured { code, .. } => code,
            Self::Io(_) => "system_network_unavailable",
            #[cfg(mobile)]
            Self::PluginInvoke(_) => "internal_error",
        }
    }
}

impl Serialize for Error {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.to_string().as_ref())
    }
}
