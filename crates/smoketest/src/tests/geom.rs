//! Geometry and polling helpers shared across smoketests.

use std::{
    thread,
    time::{Duration, Instant},
};

use objc2_app_kit::NSScreen;
use objc2_foundation::MainThreadMarker;

use crate::error::{Error, Result};

/// Convenience alias for a rectangle (x, y, w, h) in AppKit logical coords.
pub type Rect = (f64, f64, f64, f64);

/// Find the AppKit `visibleFrame` of the screen that contains `(x,y)`.
/// Fallbacks to main screen, then first screen when needed.
pub fn visible_frame_containing_point(x: f64, y: f64) -> Option<Rect> {
    let mtm = MainThreadMarker::new()?;
    // Prefer the screen whose visibleFrame contains the point
    for s in NSScreen::screens(mtm).iter() {
        let fr = s.visibleFrame();
        let sx = fr.origin.x;
        let sy = fr.origin.y;
        let sw = fr.size.width;
        let sh = fr.size.height;
        if x >= sx && x <= sx + sw && y >= sy && y <= sy + sh {
            return Some((sx, sy, sw, sh));
        }
    }
    // Fallbacks
    if let Some(scr) = NSScreen::mainScreen(mtm) {
        let r = scr.visibleFrame();
        return Some((r.origin.x, r.origin.y, r.size.width, r.size.height));
    }
    if let Some(s) = NSScreen::screens(mtm).iter().next() {
        let r = s.visibleFrame();
        return Some((r.origin.x, r.origin.y, r.size.width, r.size.height));
    }
    None
}

/// Resolve the `visibleFrame` of the screen containing the current AX
/// position of `(pid,title)`, retrying until `timeout_ms`.
pub fn resolve_vf_for_window(pid: i32, title: &str, timeout_ms: u64, poll_ms: u64) -> Option<Rect> {
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    while Instant::now() < deadline {
        if let Some((px, py)) = mac_winops::ax_window_position(pid, title)
            && let Some(vf) = visible_frame_containing_point(px, py)
        {
            return Some(vf);
        }
        thread::sleep(Duration::from_millis(poll_ms));
    }
    None
}

/// Compute the exact grid cell rectangle within a given visible frame.
#[allow(clippy::too_many_arguments)]
pub fn cell_rect(vf: Rect, cols: u32, rows: u32, col: u32, row: u32) -> Rect {
    let (vf_x, vf_y, vf_w, vf_h) = vf;
    let cols_f = cols.max(1) as f64;
    let rows_f = rows.max(1) as f64;
    let tile_w = (vf_w / cols_f).floor().max(1.0);
    let tile_h = (vf_h / rows_f).floor().max(1.0);
    let rem_w = vf_w - tile_w * (cols as f64);
    let rem_h = vf_h - tile_h * (rows as f64);

    let x_pos = vf_x + tile_w * (col as f64);
    let width = if col == cols.saturating_sub(1) {
        tile_w + rem_w
    } else {
        tile_w
    };
    let y_pos = vf_y + tile_h * (row as f64);
    let height = if row == rows.saturating_sub(1) {
        tile_h + rem_h
    } else {
        tile_h
    };
    (x_pos, y_pos, width, height)
}

/// Wait until `(pid,title)` reports an AX frame approximately equal to
/// `expected` within `eps` tolerance.
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
        if let Some(((px, py), (w, h))) = mac_winops::ax_window_frame(pid, title)
            && super::helpers::approx(px, expected.0, eps)
            && super::helpers::approx(py, expected.1, eps)
            && super::helpers::approx(w, expected.2, eps)
            && super::helpers::approx(h, expected.3, eps)
        {
            return true;
        }
        thread::sleep(Duration::from_millis(poll_ms));
    }
    false
}

/// Find the CG `WindowId` for `(pid,title)` within `timeout_ms`.
pub fn find_window_id(
    pid: i32,
    title: &str,
    timeout_ms: u64,
    poll_ms: u64,
) -> Option<mac_winops::WindowId> {
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    while Instant::now() < deadline {
        if let Some(w) = mac_winops::list_windows()
            .into_iter()
            .find(|w| w.pid == pid && w.title == title)
        {
            return Some(w.id);
        }
        thread::sleep(Duration::from_millis(poll_ms));
    }
    None
}

/// Assert that the current CG frontmost window has `expected_title` and that
/// its AX frame corresponds to the given grid cell within `eps`.
pub fn assert_frontmost_cell(
    expected_title: &str,
    vf: Rect,
    cols: u32,
    rows: u32,
    col: u32,
    row: u32,
    eps: f64,
) -> Result<()> {
    let front = mac_winops::frontmost_window()
        .ok_or_else(|| Error::InvalidState("No frontmost CG window".into()))?;
    if front.title != expected_title {
        return Err(Error::FocusNotObserved {
            timeout_ms: 1000,
            expected: format!("{} (frontmost: {})", expected_title, front.title),
        });
    }
    let ((x, y), (w, h)) = mac_winops::ax_window_frame(front.pid, &front.title)
        .ok_or_else(|| Error::InvalidState("AX frame for frontmost not available".into()))?;
    let (ex, ey, ew, eh) = cell_rect(vf, cols, rows, col, row);
    if !(super::helpers::approx(x, ex, eps)
        && super::helpers::approx(y, ey, eps)
        && super::helpers::approx(w, ew, eps)
        && super::helpers::approx(h, eh, eps))
    {
        return Err(Error::InvalidState(format!(
            "frontmost not in expected cell ({},{}): got x={:.1} y={:.1} w={:.1} h={:.1} | expected x={:.1} y={:.1} w={:.1} h={:.1}",
            col, row, x, y, w, h, ex, ey, ew, eh
        )));
    }
    Ok(())
}
