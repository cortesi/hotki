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

/// Smoketest-local result type.
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
        Error::RpcDriver(inner) => {
            eprintln!("hint: RPC driver failed: {inner}");
            match inner {
                DriverError::Connect { socket_path, .. } => {
                    eprintln!(
                        "      ensure the UI-owned server is listening at '{}'.",
                        socket_path
                    );
                    eprintln!(
                        "      check Hotki permissions and rebuild the smoketest harness if needed."
                    );
                }
                DriverError::InitTimeout { socket_path, .. } => {
                    eprintln!(
                        "      server driver did not initialize in time (socket: '{}').",
                        socket_path
                    );
                    eprintln!(
                        "      verify the UI launched, connected to its server, and loaded config."
                    );
                    eprintln!(
                        "      if startup logs report Accessibility=false, grant Accessibility to the launched Hotki binary."
                    );
                }
                DriverError::EventStreamTimeout { socket_path, .. } => {
                    eprintln!(
                        "      server accepted RPCs but sent no events on '{}'.",
                        socket_path
                    );
                    eprintln!("      verify the event forwarder and heartbeat tasks started.");
                }
                DriverError::NotInitialized => {
                    eprintln!(
                        "      the driver was used before HotkiSession finished initialization."
                    );
                }
                DriverError::BindingTimeout { ident, .. } => {
                    eprintln!(
                        "      binding '{}' was never observed; confirm config matches the test.",
                        ident
                    );
                }
                DriverError::Runtime { message } => {
                    eprintln!("      failed to create the local async runtime: {message}");
                }
                DriverError::ServerFailure { message } => {
                    eprintln!(
                        "      see logs above for server errors returned by hotki runtime: {}",
                        message
                    );
                }
            }
        }
    }
}
