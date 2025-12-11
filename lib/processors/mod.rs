mod html;
mod spans;

pub use html::HtmlProcessor;
pub use spans::SpansProcessor;

use std::sync::Arc;
use std::sync::atomic::AtomicUsize;

use axum::body::Bytes;

use crate::errors::FatalError;
use crate::languages;
use crate::daylight_generated::daylight::common;

/// The result of an enqueued highlight task. Not a Result<> because my brain is too small
/// to handle nested Result types in associated Future output times.
pub enum Outcome<T> {
    Success {
        ident: u16,
        // don't LOVE the Arc but lifetimes become quite difficult without them
        filename: Arc<str>,
        language: languages::SharedConfig,
        contents: Vec<T>,
    },
    Failure {
        ident: u16,
        filename: Arc<str>,
        language: Option<languages::SharedConfig>,
        reason: crate::errors::NonFatalError,
    },
}

impl<T> Outcome<T> {
    pub fn ident(&self) -> u16 {
        match self {
            Self::Success { ident, .. } => *ident,
            Self::Failure { ident, .. } => *ident,
        }
    }

    pub fn filename<'a>(&'a self) -> &'a str {
        match self {
            Self::Success { filename, .. } => filename.as_ref(),
            Self::Failure { .. } => Default::default(),
        }
    }

    pub fn language(&self) -> common::Language {
        match self {
            Self::Success { language, .. } => language.fb_language,
            Self::Failure { language, .. } => language.map(|l| l.fb_language).unwrap_or_default(),
        }
    }

    pub fn error_code(&self) -> common::ErrorCode {
        match self {
            Self::Success { .. } => common::ErrorCode::NoError,
            Self::Failure { reason, .. } => (*reason).into(),
        }
    }
}

/// Trait for processing highlight events into different output formats.
pub trait Processor: Send + Sync + 'static {
    type Output: Send;

    /// Process file contents and return the processed output.
    fn process(
        ident: u16,
        filename: Arc<str>,
        language: languages::SharedConfig,
        contents: Bytes,
        include_injections: bool,
        cancellation_flag: Arc<AtomicUsize>,
    ) -> Outcome<Self::Output>;

    /// Build the final HTTP response from a collection of outputs.
    fn build_response(
        outputs: Vec<Outcome<Self::Output>>,
    ) -> Result<axum::response::Response, FatalError>;
}
