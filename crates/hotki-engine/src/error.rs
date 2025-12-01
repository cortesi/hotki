use std::{io, result::Result as StdResult};

use thiserror::Error;

/// Convenient result type for the engine crate.
pub type Result<T> = StdResult<T, Error>;

/// Unified error type for the Hotki engine.
#[derive(Debug, Error)]
pub enum Error {
    /// Errors originating from the mac-hotkey layer.
    #[error("Hotkey manager error: {0}")]
    Hotkey(#[from] mac_hotkey::Error),

    /// The UI event channel has been closed by the receiver.
    #[error("UI channel closed")]
    ChannelClosed,

    /// I/O failure while performing a system operation.
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),

    /// Generic error with context.
    #[error("Engine error: {0}")]
    Msg(String),
}
