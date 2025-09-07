//! Common UI interaction utilities for tests.

use std::thread;

use crate::{config, server_drive};
// RPC driver is always preferred when available; HID is a fallback.

/// Send a single key chord using the RelayKey mechanism.
/// This is the standard way tests interact with hotki.
pub fn send_key(seq: &str) {
    if server_drive::is_ready() && server_drive::inject_key(seq) {
        return;
    }
    // fall back to HID on failure
    if let Some(ch) = mac_keycode::Chord::parse(seq) {
        let rk = relaykey::RelayKey::new_unlabeled();
        rk.key_down(0, ch.clone(), false);
        thread::sleep(config::ms(config::KEY_EVENT_DELAY_MS));
        rk.key_up(0, ch);
    }
}

/// Send a sequence of key chords with delays between them.
pub fn send_key_sequence(sequences: &[&str]) {
    if server_drive::is_ready() && server_drive::inject_sequence(sequences) {
        return;
    }
    // fall back to HID on failure
    let rk = relaykey::RelayKey::new_unlabeled();
    for s in sequences {
        if let Some(ch) = mac_keycode::Chord::parse(s) {
            rk.key_down(0, ch.clone(), false);
            thread::sleep(config::ms(config::KEY_EVENT_DELAY_MS));
            rk.key_up(0, ch);
            thread::sleep(config::ms(config::UI_ACTION_DELAY_MS));
        }
    }
}

/// Send the standard hotki activation chord (shift+cmd+0).
pub fn send_activation_chord() {
    send_key("shift+cmd+0");
}

// Deprecated: use explicit gated send_key calls in tests for reliability.
