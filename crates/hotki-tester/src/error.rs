//! Error handling for the hotki-tester crate.

use std::{io, result, time::Duration};

use thiserror::Error;
use tokio::time::error::Elapsed;

/// Convenient result type for hotki-tester operations.
pub type Result<T> = result::Result<T, Error>;

/// Errors that can occur while running the tester.
#[derive(Debug, Error)]
pub enum Error {
    /// Wrapper for standard I/O errors.
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
    /// Errors surfaced by the hotki server client.
    #[error("Hotki server error: {0}")]
    HotkiServer(#[from] hotki_server::Error),
    /// Errors encountered while performing placement operations.
    #[error("Placement error: {0}")]
    Placement(#[from] mac_winops::Error),
    /// Configuration parsing or resolution errors.
    #[error("Configuration error: {0}")]
    Config(#[from] config::Error),
    /// Failed to parse the placement directive string.
    #[error("Failed to parse placement directive: {0}")]
    DirectiveSpec(String),
    /// The backend did not become ready before the timeout elapsed.
    #[error("Backend startup timed out after {0:?}")]
    BackendStartupTimeout(Duration),
    /// No focused window could be determined from the world snapshot.
    #[error("No focused window detected in world snapshot")]
    NoFocusedWindow,
    /// No placement directives were provided to the tester command.
    #[error("No placement directives supplied; pass place(...) or place_move(...) arguments")]
    NoPlacementDirectives,
    /// Unable to resolve the Core Graphics window identifier for the focused window.
    #[error("Unable to determine window id for PID {pid}")]
    WindowIdUnavailable {
        /// Process identifier associated with the missing window id.
        pid: i32,
    },
    /// Generic error for unexpected conditions.
    #[error("{0}")]
    Other(String),
}

impl From<Elapsed> for Error {
    fn from(err: Elapsed) -> Self {
        Self::Other(format!("operation timed out: {err}"))
    }
}

impl Error {
    /// Helper to build a parse error from an arbitrary message.
    pub fn parse<M: Into<String>>(msg: M) -> Self {
        Self::DirectiveSpec(msg.into())
    }

    /// Helper for wrapping generic string errors.
    pub fn other<M: Into<String>>(msg: M) -> Self {
        Self::Other(msg.into())
    }
}
