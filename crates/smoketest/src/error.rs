use std::path::PathBuf;
use thiserror::Error;

/// Errors that can occur during smoketest execution.
#[derive(Error, Debug)]
pub enum Error {
    /// Configuration file is missing or invalid.
    #[error("missing config: {}", .0.display())]
    MissingConfig(PathBuf),

    /// The hotki binary could not be found.
    #[error("could not locate 'hotki' binary (set HOTKI_BIN or `cargo build --bin hotki`)")]
    HotkiBinNotFound,

    /// Failed to spawn a process.
    #[error("failed to launch hotki: {0}")]
    SpawnFailed(String),

    /// HUD did not become visible within the timeout period.
    #[error("HUD did not appear within {timeout_ms} ms (no HudUpdate depth>0)")]
    HudNotVisible { timeout_ms: u64 },

    /// Expected focus was not observed within the timeout period.
    #[error("did not observe matching focus title within {timeout_ms} ms (expected: '{expected}')")]
    FocusNotObserved { timeout_ms: u64, expected: String },

    /// Screen capture failed.
    #[error("failed to capture {0} window")]
    CaptureFailed(&'static str),

    /// I/O operation failed.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// Invalid test state.
    #[error("invalid test state: {0}")]
    InvalidState(String),
}

pub type Result<T> = std::result::Result<T, Error>;

/// Print helpful hints for common errors.
pub fn print_hints(err: &Error) {
    match err {
        Error::HotkiBinNotFound => {
            eprintln!("hint: set HOTKI_BIN to an existing binary or run: cargo build --bin hotki");
        }
        Error::HudNotVisible { .. } => {
            eprintln!("hint: the activation chord is sent via Accessibility (HID)");
            eprintln!(
                "      ensure the terminal/shell running smoketest is allowed under System Settings → Privacy & Security → Accessibility"
            );
            eprintln!("      also check hotki logs with --logs for server startup issues");
        }
        Error::FocusNotObserved { .. } => {
            eprintln!(
                "hint: ensure the smoketest window is frontmost (we call NSApplication.activate)"
            );
            eprintln!("      grant Accessibility permission for faster title updates (optional)");
            eprintln!("      use --logs to inspect focus watcher and HudUpdate events");
        }
        Error::CaptureFailed(_) => {
            eprintln!("hint: screencapture requires Screen Recording permission for the terminal");
            eprintln!(
                "      grant it under System Settings → Privacy & Security → Screen Recording"
            );
        }
        Error::MissingConfig(_) => {
            eprintln!(
                "hint: expected examples/test.ron relative to repo root (or pass a valid config)"
            );
        }
        Error::SpawnFailed(_) | Error::Io(_) | Error::InvalidState(_) => {
            // No specific hints for these errors
        }
    }
}
