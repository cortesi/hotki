//! Polling utilities for waiting on macOS window state.

use std::{
    thread,
    time::{Duration, Instant},
};


use crate::{
    WindowId, WindowInfo,
    window::{list_windows, list_windows_for_spaces},
};

/// Find the CoreGraphics window id associated with `(pid, title)`.
pub fn find_window_id(
    pid: i32,
    title: &str,
    timeout: Duration,
    poll_interval: Duration,
) -> Option<WindowId> {
    let start = Instant::now();
    let deadline = start
        .checked_add(timeout)
        .unwrap_or_else(|| start + timeout);
    let mut attempted_all_spaces = false;

    loop {
        let search =
            |wins: Vec<WindowInfo>| wins.into_iter().find(|w| w.pid == pid && w.title == title);

        if let Some(w) = search(list_windows()) {
            return Some(w.id);
        }

        if !attempted_all_spaces {
            if let Some(w) = search(list_windows_for_spaces(&[])) {
                return Some(w.id);
            }
            attempted_all_spaces = true;
        }

        if Instant::now() >= deadline {
            return None;
        }

        if poll_interval.is_zero() {
            thread::yield_now();
        } else if let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
            thread::sleep(poll_interval.min(remaining));
        } else {
            return None;
        }
    }
}
