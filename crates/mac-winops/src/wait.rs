//! Polling utilities for waiting on macOS window state.

use std::{
    thread,
    time::{Duration, Instant},
};

use objc2_foundation::MainThreadMarker;

use crate::{
    WindowId, ax_has_window_title, ax_window_position,
    geom::{Point, Rect},
    list_windows,
};

/// Wait until all `(pid, title)` pairs are visible via CoreGraphics or AX.
///
/// Returns `true` once every entry is observed before `timeout` expires.
/// Polls both `list_windows` and `ax_has_window_title`, sleeping for
/// `poll_interval` between iterations when non-zero.
pub fn wait_for_windows_visible(
    entries: &[(i32, &str)],
    timeout: Duration,
    poll_interval: Duration,
) -> bool {
    let start = Instant::now();
    let deadline = start.checked_add(timeout).unwrap_or_else(|| {
        // Overflow means we can treat the wait as effectively unbounded.
        Instant::now() + Duration::from_secs(24 * 60 * 60)
    });

    loop {
        if entries.iter().all(|(pid, title)| {
            let wins = list_windows();
            let cg_present = wins.iter().any(|w| w.pid == *pid && w.title == *title);
            let ax_present = ax_has_window_title(*pid, title);
            cg_present || ax_present
        }) {
            return true;
        }

        if Instant::now() >= deadline {
            return false;
        }

        if poll_interval.is_zero() {
            thread::yield_now();
        } else if let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
            thread::sleep(poll_interval.min(remaining));
        } else {
            return false;
        }
    }
}

/// Convenience wrapper using millisecond inputs for compatibility with
/// existing test helpers.
#[inline]
pub fn wait_for_windows_visible_ms(entries: &[(i32, &str)], timeout_ms: u64, poll_ms: u64) -> bool {
    wait_for_windows_visible(
        entries,
        Duration::from_millis(timeout_ms),
        Duration::from_millis(poll_ms),
    )
}

/// Resolve the visible frame containing the AX position of `(pid, title)`.
///
/// Returns the AppKit visible frame as a `Rect` when both the AX window
/// position and the screen lookup succeed before the timeout expires.
pub fn resolve_vf_for_window(
    pid: i32,
    title: &str,
    timeout: Duration,
    poll_interval: Duration,
) -> Option<Rect> {
    let mtm = MainThreadMarker::new()?;
    let start = Instant::now();
    let deadline = start
        .checked_add(timeout)
        .unwrap_or_else(|| start + timeout);

    loop {
        if let Some((px, py)) = ax_window_position(pid, title) {
            let pt = Point { x: px, y: py };
            let vf = crate::screen_util::visible_frame_containing_point(mtm, pt);
            return Some(vf);
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

/// Millisecond wrapper for `resolve_vf_for_window`.
#[inline]
pub fn resolve_vf_for_window_ms(
    pid: i32,
    title: &str,
    timeout_ms: u64,
    poll_ms: u64,
) -> Option<Rect> {
    resolve_vf_for_window(
        pid,
        title,
        Duration::from_millis(timeout_ms),
        Duration::from_millis(poll_ms),
    )
}

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

    loop {
        if let Some(w) = list_windows()
            .into_iter()
            .find(|w| w.pid == pid && w.title == title)
        {
            return Some(w.id);
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

/// Millisecond wrapper for `find_window_id`.
#[inline]
pub fn find_window_id_ms(pid: i32, title: &str, timeout_ms: u64, poll_ms: u64) -> Option<WindowId> {
    find_window_id(
        pid,
        title,
        Duration::from_millis(timeout_ms),
        Duration::from_millis(poll_ms),
    )
}
