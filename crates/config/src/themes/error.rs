use std::path::PathBuf;

use thiserror::Error;

/// Errors produced while loading, parsing, or validating a theme script.
#[derive(Debug, Error, Clone)]
pub enum ThemeError {
    /// I/O or filesystem read error.
    #[error("{message}")]
    Read {
        /// Path associated with the read error.
        path: PathBuf,
        /// Human-readable error message.
        message: String,
    },
    /// Rhai parse/runtime error with a concrete line/column location and excerpt.
    #[error("{message}")]
    Parse {
        /// Path associated with the parse error.
        path: PathBuf,
        /// 1-based line number.
        line: usize,
        /// 1-based column number.
        col: usize,
        /// Human-readable error message.
        message: String,
        /// Rendered excerpt including a caret at the error location.
        excerpt: String,
    },
    /// Validation error without a reliable source location (e.g. schema mismatch).
    #[error("{message}")]
    Validation {
        /// Path associated with the validation error.
        path: PathBuf,
        /// Optional 1-based line number.
        line: Option<usize>,
        /// Optional 1-based column number.
        col: Option<usize>,
        /// Human-readable error message.
        message: String,
        /// Optional excerpt including a caret at the error location.
        excerpt: Option<String>,
    },
}

impl From<ThemeError> for crate::Error {
    fn from(err: ThemeError) -> Self {
        match err {
            ThemeError::Read { path, message } => Self::Read {
                path: Some(path),
                message,
            },
            ThemeError::Parse {
                path,
                line,
                col,
                message,
                excerpt,
            } => Self::Parse {
                path: Some(path),
                line,
                col,
                message,
                excerpt,
            },
            ThemeError::Validation {
                path,
                line,
                col,
                message,
                excerpt,
            } => Self::Validation {
                path: Some(path),
                line,
                col,
                message,
                excerpt,
            },
        }
    }
}
