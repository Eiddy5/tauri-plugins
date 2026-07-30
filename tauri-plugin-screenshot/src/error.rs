use serde::Serialize;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ErrorCode {
    Busy,
    PermissionDenied,
    Unsupported,
    DisplayUnavailable,
    CaptureFailed,
    OverlayFailed,
    InvalidSelection,
    ResultUnavailable,
    Internal,
}

#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct Error {
    pub code: ErrorCode,
    pub message: String,
    pub recoverable: bool,
}

impl Error {
    pub fn new(code: ErrorCode, message: impl Into<String>, recoverable: bool) -> Self {
        Self {
            code,
            message: message.into(),
            recoverable,
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        Self::new(ErrorCode::Internal, error.to_string(), true)
    }
}

impl Serialize for Error {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct ErrorPayload<'a> {
            code: ErrorCode,
            message: &'a str,
            recoverable: bool,
        }

        ErrorPayload {
            code: self.code,
            message: &self.message,
            recoverable: self.recoverable,
        }
        .serialize(serializer)
    }
}
