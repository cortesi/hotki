use std::{io::Error as IoError, result::Result as StdResult};

use thiserror::Error;

/// The main error type for hotki-server operations (crate-internal)
#[derive(Error, Debug)]
pub enum Error {
    /// Error registering or managing hotkeys
    #[error("Hotkey error: {0}")]
    HotkeyOperation(String),

    /// Error in IPC communication
    #[error("IPC error: {0}")]
    Ipc(String),

    /// IO-related errors
    #[error("IO error: {0}")]
    Io(#[from] IoError),

    /// Serialization/deserialization errors
    #[error("Serialization error: {0}")]
    Serialization(String),
}

/// Convenience type alias for Results using our Error type
pub type Result<T> = StdResult<T, Error>;

// Implement conversions for common error types we encounter
impl From<rmp_serde::encode::Error> for Error {
    fn from(err: rmp_serde::encode::Error) -> Self {
        Error::Serialization(err.to_string())
    }
}

impl From<rmp_serde::decode::Error> for Error {
    fn from(err: rmp_serde::decode::Error) -> Self {
        Error::Serialization(err.to_string())
    }
}

impl From<mac_hotkey::Error> for Error {
    fn from(err: mac_hotkey::Error) -> Self {
        Error::HotkeyOperation(err.to_string())
    }
}

/// Stable RPC error codes surfaced via MRPC `ServiceError.name`.
///
/// Use `to_string()` (Display) to produce the canonical code string.
#[derive(Error, Debug, Clone, Copy, PartialEq, Eq)]
pub enum RpcErrorCode {
    #[error("ShuttingDown")]
    ShuttingDown,
    #[error("MissingParams")]
    MissingParams,
    #[error("InvalidType")]
    InvalidType,
    #[error("InvalidConfig")]
    InvalidConfig,
    #[error("MethodNotFound")]
    MethodNotFound,
    #[error("EngineInit")]
    EngineInit,
    #[error("EngineNotInitialized")]
    EngineNotInitialized,
    #[error("EngineSetConfig")]
    EngineSetConfig,
    #[error("KeyNotBound")]
    KeyNotBound,
    #[error("EngineDispatch")]
    EngineDispatch,
    #[error("WindowOffActiveSpace")]
    WindowOffActiveSpace,
}
