use std::fmt;

use serde::{Deserialize, Serialize};

/// Stable machine-readable diagnostic emitted by library and CLI surfaces.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diagnostic {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

impl Diagnostic {
    pub(crate) fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            path: None,
        }
    }

    pub(crate) fn at(mut self, path: impl Into<String>) -> Self {
        self.path = Some(path.into());
        self
    }
}

/// A source contract cannot be projected without semantic review.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GenerationError {
    pub diagnostic: Diagnostic,
}

impl GenerationError {
    pub(crate) fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            diagnostic: Diagnostic::new(code, message),
        }
    }

    pub(crate) fn at(
        code: &'static str,
        path: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            diagnostic: Diagnostic::new(code, message).at(path),
        }
    }
}

impl fmt::Display for GenerationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.diagnostic.message)
    }
}

impl std::error::Error for GenerationError {}

pub(crate) type Result<T> = std::result::Result<T, GenerationError>;
