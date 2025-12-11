use axum::response::IntoResponse;
use http::StatusCode;
use opentelemetry::trace;
use thiserror::Error;
use tokio::time::Duration;
use tracing_opentelemetry::OpenTelemetrySpanExt;
use tree_sitter_highlight as ts;

use crate::daylight_generated::daylight::common;

/// Hard errors (those that fail with a non-200 HTTP error).
#[derive(Debug, Error)]
pub enum FatalError {
    #[error("Decoding request failed")]
    DecodeError(#[from] flatbuffers::InvalidFlatbuffer),
    #[error("Timeout too large (max supported: {max}ms)", max = .0.as_millis())]
    TimeoutTooLarge(Duration),
}

impl IntoResponse for FatalError {
    fn into_response(self) -> axum::response::Response {
        (StatusCode::BAD_REQUEST, self.to_string()).into_response()
    }
}

/// Soft errors (those that might live alongside successful results).
#[derive(Clone, Copy, Debug, Error)]
pub enum NonFatalError {
    #[error("Cancelled")]
    Cancelled,
    #[error("Empty file, nothing to do")]
    EmptyFile,
    #[error("File too large (limit: 256MB)")]
    FileTooLarge,
    #[error("Invalid or unknown language")]
    InvalidLanguage,
    #[error("Internal threading error")]
    ThreadError,
    #[error("Timed out")]
    TimedOut,
    #[error("Unknown error")]
    UnknownError,
}

impl NonFatalError {
    pub fn record_in_span(&self) {
        tracing::Span::current().set_status(trace::Status::Error {
            description: self.to_string().into(),
        });
    }
}

impl From<ts::Error> for NonFatalError {
    fn from(value: ts::Error) -> Self {
        match value {
            ts::Error::Cancelled => Self::TimedOut,
            ts::Error::InvalidLanguage => Self::InvalidLanguage,
            ts::Error::Unknown => Self::UnknownError,
        }
    }
}

impl From<tokio::task::JoinError> for NonFatalError {
    fn from(err: tokio::task::JoinError) -> Self {
        if err.is_cancelled() {
            Self::Cancelled
        } else {
            Self::UnknownError
        }
    }
}

impl Into<common::ErrorCode> for NonFatalError {
    fn into(self) -> common::ErrorCode {
        match self {
            Self::TimedOut | Self::Cancelled => common::ErrorCode::TimedOut,
            Self::ThreadError | Self::UnknownError => common::ErrorCode::UnknownError,
            Self::InvalidLanguage => common::ErrorCode::UnknownLanguage,
            Self::FileTooLarge => common::ErrorCode::FileTooLarge,
            Self::EmptyFile => common::ErrorCode::NoError,
        }
    }
}
