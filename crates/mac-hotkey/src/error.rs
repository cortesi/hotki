//! Error types and result alias for the mac-hotkey crate.
use std::result::Result as StdResult;

use thiserror::Error;

/// Convenient result type used throughout this crate.
pub type Result<T> = StdResult<T, Error>;

/// Error variants produced by this crate.
#[derive(Error, Debug)]
pub enum Error {
    /// Underlying OS provided an error.
    #[error("OS error: {0}")]
    OsError(String),
    /// Event tap could not be created or initialized.
    #[error("Event tap failed to start")]
    EventTapStart,
    /// Missing or denied system permission.
    #[error("Permission denied: {0}")]
    PermissionDenied(&'static str),
    /// No active registration exists for the provided id.
    #[error("Invalid registration id")]
    InvalidId,
}
