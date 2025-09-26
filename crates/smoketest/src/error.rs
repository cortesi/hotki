use std::{io, result::Result as StdResult};

use thiserror::Error;

use crate::server_drive::DriverError;

/// Errors that can occur during smoketest execution.
#[derive(Error, Debug)]
pub enum Error {
    /// The hotki binary could not be found.
    #[error("could not locate 'hotki' binary (set HOTKI_BIN or `cargo build --bin hotki`)")]
    HotkiBinNotFound,

    /// Failed to spawn a process.
    #[error("failed to launch hotki: {0}")]
    SpawnFailed(String),

    /// HUD did not become visible within the timeout period.
    #[error("HUD did not appear within {timeout_ms} ms (no HudUpdate depth>0)")]
    HudNotVisible {
        /// Timeout in milliseconds
        timeout_ms: u64,
    },

    /// MRPC event stream closed unexpectedly while a smoketest was running.
    #[error("IPC disconnected unexpectedly while {during}")]
    IpcDisconnected {
        /// Context description of what was running when IPC disconnected
        during: &'static str,
    },

    /// MRPC driver operations failed while interacting with hotki-server.
    #[error("RPC driver failure: {0}")]
    RpcDriver(#[from] DriverError),

    /// I/O operation failed.
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),

    /// Invalid test state.
    #[error("invalid test state: {0}")]
    InvalidState(String),
}

pub type Result<T> = StdResult<T, Error>;

/// Print helpful hints for common errors.
pub fn print_hints(err: &Error) {
    match err {
        Error::HotkiBinNotFound => {
            eprintln!("hint: set HOTKI_BIN to an existing binary or run: cargo build --bin hotki");
        }
        Error::HudNotVisible { .. } => {
            eprintln!("hint: we inject the activation chord via RPC");
            eprintln!("      check that the server started (use --logs) and bindings are ready");
            eprintln!("      also ensure Accessibility is granted for best reliability");
        }
        Error::SpawnFailed(_) | Error::Io(_) | Error::InvalidState(_) => {
            // No specific hints for these errors
        }
        Error::IpcDisconnected { .. } => {
            eprintln!("hint: backend crashed or exited; run with --logs to capture cause");
            eprintln!(
                "      if this happened during fullscreen, check macOS accessibility issues and AXFullScreen support"
            );
        }
        Error::RpcDriver(inner) => {
            eprintln!("hint: RPC driver failed: {inner}");
            match inner {
                DriverError::Connect { socket_path, .. } => {
                    eprintln!(
                        "      ensure the UI bridge listener is running at '{}'.",
                        socket_path
                    );
                    eprintln!(
                        "      check permissions and rebuild the smoketest harness if needed."
                    );
                }
                DriverError::InitTimeout { socket_path, .. } => {
                    eprintln!(
                        "      bridge connection did not initialize in time (socket: '{}').",
                        socket_path
                    );
                    eprintln!("      verify the backend launched and the socket path is correct.");
                }
                DriverError::NotInitialized => {
                    eprintln!(
                        "      the driver was used before calling TestContext::ensure_rpc_ready()."
                    );
                }
                DriverError::BindingTimeout { ident, .. } => {
                    eprintln!(
                        "      binding '{}' was never observed; confirm config matches the test.",
                        ident
                    );
                }
                DriverError::BridgeFailure { message } => {
                    eprintln!(
                        "      see logs above for bridge errors returned by hotki runtime: {}",
                        message
                    );
                }
                DriverError::Io { source } => {
                    eprintln!("      IO error talking to the bridge: {source}");
                }
            }
        }
    }
}
