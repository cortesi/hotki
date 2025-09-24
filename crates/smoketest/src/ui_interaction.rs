//! Common UI interaction utilities for tests.

use crate::{error::Result, server_drive};

/// Send a single key chord using the RelayKey mechanism.
/// This is the standard way tests interact with hotki.
pub fn send_key(seq: &str) -> Result<()> {
    server_drive::inject_key(seq)?;
    Ok(())
}
