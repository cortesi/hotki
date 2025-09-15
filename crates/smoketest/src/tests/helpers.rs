//! Shared helpers for smoketests to keep tests concise.

use std::{
    thread,
    time::{Duration, Instant},
};

use crate::{config, server_drive};

/// Approximate float equality within `eps`.
pub fn approx(a: f64, b: f64, eps: f64) -> bool {
    (a - b).abs() <= eps
}

/// Wait until all `(pid,title)` pairs are visible via CG or AX.
pub fn wait_for_windows_visible(entries: &[(i32, &str)], timeout_ms: u64) -> bool {
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    while Instant::now() < deadline {
        let wins = mac_winops::list_windows();
        let all_found = entries.iter().all(|(pid, title)| {
            let cg_present = wins.iter().any(|w| w.pid == *pid && w.title == *title);
            let ax_present = mac_winops::ax_has_window_title(*pid, title);
            cg_present || ax_present
        });
        if all_found {
            return true;
        }
        thread::sleep(config::ms(config::POLL_INTERVAL_MS));
    }
    false
}

/// Wait until the backend-reported focused window title equals `expected_title`.
///
/// This uses the lightweight shared RPC driver if available; otherwise, it
/// returns `false` without side effects. Prefer `wait_for_frontmost_title`
/// for acceptance checks that must reflect the actual CG frontmost window.
pub fn wait_for_backend_focused_title(expected_title: &str, timeout_ms: u64) -> bool {
    if server_drive::is_ready() {
        server_drive::wait_for_focused_title(expected_title, timeout_ms)
    } else {
        false
    }
}
