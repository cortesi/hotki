use std::{io::Error as IoError, result::Result as StdResult, str::FromStr};

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

    /// Spawned server process exited before the client could connect.
    #[error("Server process exited before connection was ready on {socket_path}: {status}")]
    ServerExitedBeforeConnect {
        /// Socket path the client was trying to connect to.
        socket_path: String,
        /// Process exit status as reported by the operating system.
        status: String,
    },

    /// Typed error returned by an RPC service method.
    #[error("{method} request failed: service error {code}: {message}")]
    Rpc {
        /// RPC method that returned the error.
        method: String,
        /// Stable service error code.
        code: RpcErrorCode,
        /// Human-readable service error payload.
        message: String,
    },

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
    /// Server is shutting down and cannot accept the request.
    #[error("ShuttingDown")]
    ShuttingDown,
    /// Required request parameters were missing.
    #[error("MissingParams")]
    MissingParams,
    /// Request parameter or payload type was invalid.
    #[error("InvalidType")]
    InvalidType,
    /// Requested config update was invalid.
    #[error("InvalidConfig")]
    InvalidConfig,
    /// Requested RPC method is unknown.
    #[error("MethodNotFound")]
    MethodNotFound,
    /// Engine rejected a config update.
    #[error("EngineSetConfig")]
    EngineSetConfig,
    /// Requested key identifier is not currently bound.
    #[error("KeyNotBound")]
    KeyNotBound,
    /// Engine dispatch failed while handling a key injection.
    #[error("EngineDispatch")]
    EngineDispatch,
}

impl RpcErrorCode {
    /// Parse the stable MRPC service-error name into a typed code.
    pub fn from_service_name(name: &str) -> Option<Self> {
        Some(match name {
            "ShuttingDown" => Self::ShuttingDown,
            "MissingParams" => Self::MissingParams,
            "InvalidType" => Self::InvalidType,
            "InvalidConfig" => Self::InvalidConfig,
            "MethodNotFound" => Self::MethodNotFound,
            "EngineSetConfig" => Self::EngineSetConfig,
            "KeyNotBound" => Self::KeyNotBound,
            "EngineDispatch" => Self::EngineDispatch,
            _ => return None,
        })
    }
}

impl FromStr for RpcErrorCode {
    type Err = ();

    fn from_str(name: &str) -> StdResult<Self, Self::Err> {
        Self::from_service_name(name).ok_or(())
    }
}

#[cfg(test)]
mod tests {
    use super::RpcErrorCode;

    #[test]
    fn rpc_error_code_parses_stable_service_names() {
        assert_eq!(
            RpcErrorCode::from_service_name("KeyNotBound"),
            Some(RpcErrorCode::KeyNotBound)
        );
        assert_eq!(RpcErrorCode::from_service_name("Other"), None);
    }
}
