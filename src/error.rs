use serde::{Deserialize, Serialize};
use std::fmt;
use thiserror::Error;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ErrorCode {
    InvalidArgument,
    NotFound,
    AlreadyClosed,
    ConnectTimeout,
    ConnectFailed,
    AuthFailed,
    HostkeyMismatch,
    IoError,
    RemoteClosed,
    ExecTimeout,
    Unsupported,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiError {
    pub error_code: ErrorCode,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<String>,
}

impl ApiError {
    pub fn new(error_code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            error_code,
            message: message.into(),
            details: None,
        }
    }

    pub fn with_details(mut self, details: impl Into<String>) -> Self {
        self.details = Some(details.into());
        self
    }
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.error_code, self.message)
    }
}

#[derive(Debug, Error)]
pub enum PtyError {
    #[error("{0}")]
    Api(ApiError),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("Regex error: {0}")]
    Regex(#[from] regex::Error),
    #[error("Timeout")]
    Timeout,
}

impl From<ApiError> for PtyError {
    fn from(value: ApiError) -> Self {
        Self::Api(value)
    }
}

impl ErrorCode {
    pub fn as_str(&self) -> &'static str {
        match self {
            ErrorCode::InvalidArgument => "INVALID_ARGUMENT",
            ErrorCode::NotFound => "NOT_FOUND",
            ErrorCode::AlreadyClosed => "ALREADY_CLOSED",
            ErrorCode::ConnectTimeout => "CONNECT_TIMEOUT",
            ErrorCode::ConnectFailed => "CONNECT_FAILED",
            ErrorCode::AuthFailed => "AUTH_FAILED",
            ErrorCode::HostkeyMismatch => "HOSTKEY_MISMATCH",
            ErrorCode::IoError => "IO_ERROR",
            ErrorCode::RemoteClosed => "REMOTE_CLOSED",
            ErrorCode::ExecTimeout => "EXEC_TIMEOUT",
            ErrorCode::Unsupported => "UNSUPPORTED",
        }
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

pub type PtyResult<T> = Result<T, PtyError>;
