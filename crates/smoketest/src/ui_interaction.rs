//! Common UI interaction utilities for tests.

use crate::{error::Result, server_drive};
// RPC driver only: tests now drive via injection.

/// Send a single key chord using the RelayKey mechanism.
/// This is the standard way tests interact with hotki.
pub fn send_key(seq: &str) -> Result<()> {
    server_drive::inject_key(seq)?;
    Ok(())
}

/// Send a sequence of key chords with delays between them.
pub fn send_key_sequence(sequences: &[&str]) -> Result<()> {
    server_drive::inject_sequence(sequences)?;
    Ok(())
}

/// Send the standard hotki activation chord (shift+cmd+0).
pub fn send_activation_chord() -> Result<()> {
    send_key("shift+cmd+0")
}
