//! Repro for move-within-grid when the app disables resizing entirely.
//!
//! We spawn a helper window with the NSResizable style mask removed, then
//! attempt to move it within a 4x4 grid. Since AXSize is not settable, the
//! operation should fall back to anchoring the legal size and still update the
//! window position within the grid.

use std::{
    thread,
    time::{Duration, Instant},
};

use hotki_world_ids::WorldWindowId;

use crate::{
    config,
    error::{Error, Result},
    helper_window::{HelperWindow, HelperWindowBuilder},
    tests::fixtures::{self, Rect},
};

/// Run the non-resizable move smoketest to verify anchored fallback when AXSize is not settable.
pub fn run_place_move_nonresizable_test(timeout_ms: u64, with_logs: bool) -> Result<()> {
    let title = config::test_title("place-move-nonresizable");
    let lifetime = timeout_ms.saturating_add(config::HELPER_WINDOW.extra_time_ms);
    // Spawn helper: start in TL of 4x4 grid with a size larger than a single cell
    let builder = HelperWindowBuilder::new(title.clone())
        .with_time_ms(lifetime)
        .with_label_text("NR")
        .with_grid(4, 4, 0, 0)
        .with_size(1000.0, 700.0)
        .with_nonresizable(true);
    let mut helper = HelperWindow::spawn_frontmost_with_builder(
        builder,
        &title,
        timeout_ms,
        config::PLACE.poll_ms,
        with_logs,
    )?;
    let pid = helper.pid;

    // Establish visible frame at current center
    let ((ax, ay), _) = mac_winops::ax_window_frame(pid, &title)
        .ok_or_else(|| Error::InvalidState("AX frame for helper unavailable".into()))?;
    let vf = fixtures::visible_frame_containing_point(ax, ay)
        .ok_or_else(|| Error::InvalidState("visibleFrame not resolved".into()))?;

    // Move right by one cell
    let id = fixtures::find_window_id(
        pid,
        &title,
        config::DEFAULTS.timeout_ms,
        config::PLACE.poll_ms,
    )
    .ok_or_else(|| Error::InvalidState("failed to resolve WindowId for helper".into()))?;
    mac_winops::request_place_move_grid(
        WorldWindowId::new(pid, id),
        4,
        4,
        mac_winops::MoveDir::Right,
    )
    .map_err(|e| Error::SpawnFailed(format!("request_place_move_grid failed: {}", e)))?;
    mac_winops::drain_main_ops();

    // Expectation: x,y align to the next cell's origin (left+bottom flush);
    // width/height remain at or above cell size because resizing is disabled.
    let expected1 = fixtures::cell_rect(vf, 4, 4, 1, 0);
    let eps = config::PLACE.eps;
    let deadline = Instant::now() + Duration::from_millis(config::PLACE.step_timeout_ms);
    let mut ok = false;
    while Instant::now() < deadline {
        if let Some(((x, y), (w, h))) = mac_winops::ax_window_frame(pid, &title)
            && fixtures::approx(x, expected1.x, eps)
            && fixtures::approx(y, expected1.y, eps)
            && w >= expected1.w - eps
            && h >= expected1.h - eps
        {
            ok = true;
            break;
        }
        thread::sleep(Duration::from_millis(config::PLACE.poll_ms));
    }
    if !ok {
        let actual = mac_winops::ax_window_frame(helper.pid, &title)
            .map(|((x, y), (w, h))| Rect::new(x, y, w, h));
        return Err(Error::InvalidState(match actual {
            Some(actual) => format!(
                "place-move-nonresizable mismatch (expected col=1 anchors; ex x={:.1} y={:.1} w>={:.1} h>={:.1}; got x={:.1} y={:.1} w={:.1} h={:.1})",
                expected1.x,
                expected1.y,
                expected1.w,
                expected1.h,
                actual.x,
                actual.y,
                actual.w,
                actual.h
            ),
            None => "place-move-nonresizable mismatch (frame unavailable)".into(),
        }));
    }

    if let Err(_e) = helper.kill_and_wait() {}
    Ok(())
}
