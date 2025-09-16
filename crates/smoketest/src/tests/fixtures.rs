//! Shared smoketest fixtures and helpers.

use std::{
    thread,
    time::{Duration, Instant},
};

pub use mac_winops::{Rect, WindowId};
use mac_winops::{approx_eq_eps, cell_rect as win_cell_rect, screen, wait};

use crate::{
    config,
    error::{Error, Result},
    server_drive, world,
};

/// Approximate float equality within `eps` tolerance.
#[inline]
pub fn approx(a: f64, b: f64, eps: f64) -> bool {
    approx_eq_eps(a, b, eps)
}

/// Wait until all `(pid, title)` pairs are visible via CG or AX.
#[inline]
pub fn wait_for_windows_visible(entries: &[(i32, &str)], timeout_ms: u64) -> bool {
    wait::wait_for_windows_visible(
        entries,
        config::ms(timeout_ms),
        config::ms(config::INPUT_DELAYS.poll_interval_ms),
    )
}

/// Wait until the backend focus reporter sees `expected_title`.
#[inline]
pub fn wait_for_backend_focused_title(expected_title: &str, timeout_ms: u64) -> Result<()> {
    server_drive::wait_for_focused_title(expected_title, timeout_ms)?;
    Ok(())
}

/// Resolve the visible frame containing the current AX position of `(pid, title)`.
#[inline]
pub fn resolve_vf_for_window(pid: i32, title: &str, timeout_ms: u64, poll_ms: u64) -> Option<Rect> {
    wait::resolve_vf_for_window(
        pid,
        title,
        Duration::from_millis(timeout_ms),
        Duration::from_millis(poll_ms),
    )
}

/// Find the CoreGraphics window id for `(pid, title)` within `timeout_ms`.
#[inline]
pub fn find_window_id(pid: i32, title: &str, timeout_ms: u64, poll_ms: u64) -> Option<WindowId> {
    wait::find_window_id(
        pid,
        title,
        Duration::from_millis(timeout_ms),
        Duration::from_millis(poll_ms),
    )
}

/// Compute the exact grid cell rectangle within a given visible frame.
#[inline]
pub fn cell_rect(vf: Rect, cols: u32, rows: u32, col: u32, row: u32) -> Rect {
    win_cell_rect(vf, cols, rows, col, row)
}

/// Wait until `(pid,title)` reports an AX frame approximately equal to `expected`.
pub fn wait_for_expected_frame(
    pid: i32,
    title: &str,
    expected: Rect,
    eps: f64,
    timeout_ms: u64,
    poll_ms: u64,
) -> bool {
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    while Instant::now() < deadline {
        if let Some(((px, py), (w, h))) = mac_winops::ax_window_frame(pid, title) {
            let actual = Rect::new(px, py, w, h);
            if actual.approx_eq(&expected, eps) {
                return true;
            }
        }
        thread::sleep(Duration::from_millis(poll_ms));
    }
    false
}

/// Resolve the visible frame for the screen containing `(x, y)`.
#[inline]
pub fn visible_frame_containing_point(x: f64, y: f64) -> Option<Rect> {
    screen::visible_frame_containing_point(x, y)
}

/// Assert that the frontmost window matches `expected_title` and occupies the grid cell.
pub fn assert_frontmost_cell(
    expected_title: &str,
    vf: Rect,
    cols: u32,
    rows: u32,
    col: u32,
    row: u32,
    eps: f64,
) -> Result<()> {
    let front = world::frontmost_window_opt()
        .ok_or_else(|| Error::InvalidState("No frontmost world window".into()))?;
    if front.title != expected_title {
        return Err(Error::FocusNotObserved {
            timeout_ms: 1000,
            expected: format!("{} (frontmost: {})", expected_title, front.title),
        });
    }
    let ((x, y), (w, h)) = mac_winops::ax_window_frame(front.pid, &front.title)
        .ok_or_else(|| Error::InvalidState("AX frame for frontmost not available".into()))?;
    let expected = win_cell_rect(vf, cols, rows, col, row);
    let actual = Rect::new(x, y, w, h);
    if !actual.approx_eq(&expected, eps) {
        return Err(Error::InvalidState(format!(
            "frontmost not in expected cell ({},{}): got x={:.1} y={:.1} w={:.1} h={:.1} | expected x={:.1} y={:.1} w={:.1} h={:.1}",
            col,
            row,
            actual.x,
            actual.y,
            actual.w,
            actual.h,
            expected.x,
            expected.y,
            expected.w,
            expected.h
        )));
    }
    Ok(())
}
