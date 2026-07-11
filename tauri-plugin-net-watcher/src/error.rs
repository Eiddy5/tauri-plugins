use serde::{
    ser::{SerializeStruct, Serializer},
    Serialize,
};

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

    pub fn message(&self) -> String {
        match self {
            Self::Structured { message, .. } => message.clone(),
            Self::Io(error) => error.to_string(),
            #[cfg(mobile)]
            Self::PluginInvoke(error) => error.to_string(),
        }
    }
}

impl Serialize for Error {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("Error", 2)?;
        state.serialize_field("code", self.code())?;
        state.serialize_field("message", &self.message())?;
        state.end()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn structured_errors_serialize_with_code_and_message() {
        let value = serde_json::to_value(Error::invalid_config("bad")).unwrap();

        assert_eq!(value["code"], "invalid_config");
        assert!(value["message"].as_str().is_some());
    }
}
