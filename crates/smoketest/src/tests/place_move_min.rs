//! Repro for move-within-grid when the app enforces a minimum height.
//!
//! We spawn a helper window that refuses to shrink below a configured
//! `min_size` and then attempt to move it within a 4x4 grid. The target cell
//! height is smaller than the minimum, matching the Brave case reported by a
//! user. The expectation is that the move succeeds by anchoring the legal
//! size: left/right movement changes X while keeping the bottom edge flush,
//! even if H > cell height.

use std::{
    cmp, thread,
    time::{Duration, Instant},
};

use crate::{
    config,
    error::{Error, Result},
    helper_window::{HelperWindow, HelperWindowBuilder},
    tests::fixtures::{self, Rect},
};

/// Run the move-with-min-size smoketest to verify anchoring with height limits.
pub fn run_place_move_min_test(timeout_ms: u64, with_logs: bool) -> Result<()> {
    // Create a helper with a minimum height strictly larger than a 4x4 cell
    // on typical 1440p/2x screens. We pick 380 to exceed ~337px cell height
    // at 1350 VF height.
    let title = config::test_title("place-move-min");
    let lifetime = timeout_ms.saturating_add(config::HELPER_WINDOW.extra_time_ms);
    let builder = HelperWindowBuilder::new(title.clone())
        .with_time_ms(lifetime)
        .with_label_text("MIN")
        .with_min_size(320.0, 380.0)
        // Start in top-left grid cell of a 4x4 grid
        .with_grid(4, 4, 0, 0);
    let mut helper = HelperWindow::spawn_frontmost_with_builder(
        builder,
        &title,
        cmp::min(timeout_ms, config::HIDE.first_window_max_ms),
        config::PLACE.poll_ms,
        with_logs,
    )?;
    let pid = helper.pid;
    let ((ax, ay), _) = mac_winops::ax_window_frame(pid, &title)
        .ok_or_else(|| Error::InvalidState("AX frame for helper unavailable".into()))?;
    let vf = fixtures::visible_frame_containing_point(ax, ay)
        .ok_or_else(|| Error::InvalidState("visibleFrame not resolved".into()))?;

    // Verify initial cell anchors (left+bottom) — accept H >= cell height
    let expected0 = fixtures::cell_rect(vf, 4, 4, 0, 0);
    if let Some(((x, y), (w, h))) = mac_winops::ax_window_frame(pid, &title) {
        let eps = config::PLACE.eps;
        if !(fixtures::approx(x, expected0.x, eps)
            && fixtures::approx(y, expected0.y, eps)
            && fixtures::approx(w, expected0.w, eps)
            && h >= expected0.h - eps)
        {
            return Err(Error::InvalidState(format!(
                "initial placement not anchored (ex x={:.1} y={:.1} w={:.1} h>={:.1}; got x={:.1} y={:.1} w={:.1} h={:.1})",
                expected0.x, expected0.y, expected0.w, expected0.h, x, y, w, h
            )));
        }
    }

    // Find the window id and request move right by one cell in same 4x4 grid
    let id = fixtures::find_window_id(
        pid,
        &title,
        config::DEFAULTS.timeout_ms,
        config::PLACE.poll_ms,
    )
    .ok_or_else(|| Error::InvalidState("failed to resolve WindowId for helper".into()))?;
    mac_winops::request_place_move_grid(id, 4, 4, mac_winops::MoveDir::Right)
        .map_err(|e| Error::SpawnFailed(format!("request_place_move_grid failed: {}", e)))?;
    // Drain the main ops queue on the main thread to apply the move.
    mac_winops::drain_main_ops();

    // Expected anchors after moving right: x changes to col=1; bottom flush; width equals cell width; height >= cell height.
    let expected1 = fixtures::cell_rect(vf, 4, 4, 1, 0);
    let eps = config::PLACE.eps;
    let deadline = Instant::now() + Duration::from_millis(config::PLACE.step_timeout_ms);
    let mut ok = false;
    while Instant::now() < deadline {
        if let Some(((x, y), (w, h))) = mac_winops::ax_window_frame(pid, &title)
            && fixtures::approx(x, expected1.x, eps)
            && fixtures::approx(y, expected1.y, eps)
            && fixtures::approx(w, expected1.w, eps)
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
                "place-move-min mismatch (expected col=1 anchors; ex x={:.1} y={:.1} w={:.1} h>={:.1}; got x={:.1} y={:.1} w={:.1} h={:.1})",
                expected1.x,
                expected1.y,
                expected1.w,
                expected1.h,
                actual.x,
                actual.y,
                actual.w,
                actual.h
            ),
            None => "place-move-min mismatch (frame unavailable)".into(),
        }));
    }

    if let Err(_e) = helper.kill_and_wait() {}
    Ok(())
}
