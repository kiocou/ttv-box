use serde::Serialize;
use serde_json::Value;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("invalid input: {0}")]
    InvalidInput(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("storage error: {0}")]
    Storage(String),
    #[error("provider error: {0}")]
    Provider(String),
    #[error("playback error: {0}")]
    Playback(String),
    #[error("runtime error: {0}")]
    Runtime(String),
    #[error("security error: {0}")]
    Security(String),
    #[error("internal error: {0}")]
    Internal(String),
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IpcError {
    pub code: String,
    pub message: String,
    pub retryable: bool,
    pub request_id: Option<String>,
    pub details: Value,
}

impl AppError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidInput(_) => "invalid_input",
            Self::NotFound(_) => "not_found",
            Self::Storage(_) => "storage_error",
            Self::Provider(_) => "provider_error",
            Self::Playback(_) => "playback_error",
            Self::Runtime(_) => "runtime_error",
            Self::Security(_) => "security_error",
            Self::Internal(_) => "internal_error",
        }
    }

    pub fn retryable(&self) -> bool {
        matches!(
            self,
            Self::Storage(_) | Self::Provider(_) | Self::Runtime(_)
        )
    }
}

impl From<AppError> for IpcError {
    fn from(value: AppError) -> Self {
        Self {
            code: value.code().to_owned(),
            message: value.to_string(),
            retryable: value.retryable(),
            request_id: None,
            details: Value::Object(Default::default()),
        }
    }
}

impl From<rusqlite::Error> for AppError {
    fn from(value: rusqlite::Error) -> Self {
        Self::Storage(value.to_string())
    }
}

impl From<std::io::Error> for AppError {
    fn from(value: std::io::Error) -> Self {
        Self::Internal(value.to_string())
    }
}

impl From<serde_json::Error> for AppError {
    fn from(value: serde_json::Error) -> Self {
        Self::Internal(value.to_string())
    }
}
