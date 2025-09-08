//! Common UI interaction utilities for tests.

use crate::server_drive;
// RPC driver only: tests now drive via injection.

/// Send a single key chord using the RelayKey mechanism.
/// This is the standard way tests interact with hotki.
pub fn send_key(seq: &str) {
    let ok = server_drive::inject_key(seq);
    eprintln!("[send_key] inject {} -> {}", seq, ok);
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
