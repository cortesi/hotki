//! Common UI interaction utilities for tests.

use crate::server_drive;
// RPC driver only: tests now drive via injection.

/// Send a single key chord using the RelayKey mechanism.
/// This is the standard way tests interact with hotki.
pub fn send_key(seq: &str) {
    let ok = server_drive::inject_key(seq);
    let _ = ok;
}

/// Send a sequence of key chords with delays between them.
pub fn send_key_sequence(sequences: &[&str]) {
    let _ = server_drive::inject_sequence(sequences);
}

/// Send the standard hotki activation chord (shift+cmd+0).
pub fn send_activation_chord() {
    send_key("shift+cmd+0");
}

// Deprecated: use explicit gated send_key calls in tests for reliability.

/// Wait for a binding ident if the RPC driver is ready.
/// Returns whether the ident was observed within the timeout when attempted.
pub fn wait_for_ident_if_ready(ident: &str, timeout_ms: u64) -> bool {
    if crate::server_drive::is_ready() {
        return crate::server_drive::wait_for_ident(ident, timeout_ms);
    }
    false
}
