use std::{io::Error as IoError, result::Result as StdResult};

use hotki_protocol::rpc::RpcFailure;
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
    #[error("{method} request failed: {failure}")]
    Rpc {
        /// RPC method that returned the error.
        method: String,
        /// Typed service error including readable and structured data.
        failure: Box<RpcFailure>,
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
