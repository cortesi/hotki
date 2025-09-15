//! Error types and result alias for the relaykey crate.
use std::result::Result as StdResult;

use thiserror::Error;

/// Crate-local `Result` alias using the relay error type.
pub type Result<T> = StdResult<T, Error>;

/// Errors that can occur while synthesizing or posting events.
#[derive(Error, Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// Failure creating a CoreGraphics event source.
    #[error("Failed to create CGEventSource")]
    EventSource,
    /// Failure creating a CoreGraphics keyboard event.
    #[error("Failed to create CGEvent")]
    EventCreate,
    /// Required Accessibility permission is missing.
    #[error("Permission denied: {0}")]
    PermissionDenied(&'static str),
}
