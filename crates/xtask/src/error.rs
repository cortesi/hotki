use std::{
    io, path::PathBuf, process::ExitStatus, result::Result as StdResult, string::FromUtf8Error,
};

use thiserror::Error;

/// A shared `Result` type for `xtask`.
pub type Result<T> = StdResult<T, Error>;

/// Errors for `xtask`.
#[derive(Debug, Error)]
pub enum Error {
    /// The workspace root could not be derived from the current binary.
    #[error("could not determine workspace root")]
    WorkspaceRootNotFound,

    /// A filesystem error.
    #[error("io error at {path}: {source}")]
    Io {
        /// The relevant path.
        path: PathBuf,
        /// The underlying error.
        source: io::Error,
    },

    /// A file that should be UTF-8 was not.
    #[error("utf-8 error at {path}: {source}")]
    Utf8 {
        /// The relevant path.
        path: PathBuf,
        /// The underlying error.
        source: FromUtf8Error,
    },

    /// A command could not be started.
    #[error("failed to start command {program}: {source}")]
    CommandStart {
        /// The command being executed.
        program: String,
        /// The underlying error.
        source: io::Error,
    },

    /// A command exited unsuccessfully.
    #[error("command failed: {program} (status {status})")]
    CommandFailed {
        /// The command being executed.
        program: String,
        /// The exit status.
        status: ExitStatus,
    },

    /// The workspace Cargo.toml did not contain a version.
    #[error("could not determine workspace version from {path}")]
    MissingWorkspaceVersion {
        /// The path to the workspace manifest.
        path: PathBuf,
    },
}
