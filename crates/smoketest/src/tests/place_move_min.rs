//! Repro for move-within-grid when the app enforces a minimum height.
//!
//! We spawn a helper window that refuses to shrink below a configured
//! `min_size` and then attempt to move it within a 4x4 grid. The target cell
//! height is smaller than the minimum, matching the Brave case reported by a
//! user. The expectation is that the move succeeds by anchoring the legal
//! size: left/right movement changes X while keeping the bottom edge flush,
//! even if H > cell height.

use crate::{
    config,
    error::{Error, Result},
    process::HelperWindowBuilder,
    tests::{geom, helpers},
};

pub fn run_place_move_min_test(timeout_ms: u64, _with_logs: bool) -> Result<()> {
    // Create a helper with a minimum height strictly larger than a 4x4 cell
    // on typical 1440p/2x screens. We pick 380 to exceed ~337px cell height
    // at 1350 VF height.
    let title = crate::config::test_title("place-move-min");
    let lifetime = timeout_ms.saturating_add(config::HELPER_WINDOW_EXTRA_TIME_MS);
    let builder = HelperWindowBuilder::new(title.clone())
        .with_time_ms(lifetime)
        .with_label_text("MIN")
        .with_min_size(320.0, 380.0)
        // Start in top-left grid cell of a 4x4 grid
        .with_grid(4, 4, 0, 0);
    let mut helper = helpers::HelperWindow::spawn_frontmost_with_builder(
        builder,
        title.clone(),
        std::cmp::min(timeout_ms, config::HIDE_FIRST_WINDOW_MAX_MS),
        config::PLACE_POLL_MS,
    )?;
    let pid = helper.pid;
    let ((ax, ay), _) = mac_winops::ax_window_frame(pid, &title)
        .ok_or_else(|| Error::InvalidState("AX frame for helper unavailable".into()))?;
    let vf = geom::visible_frame_containing_point(ax, ay)
        .ok_or_else(|| Error::InvalidState("visibleFrame not resolved".into()))?;

    // Verify initial cell anchors (left+bottom) — accept H >= cell height
    let (ex0, ey0, ew0, eh0) = geom::cell_rect(vf, 4, 4, 0, 0);
    if let Some(((x, y), (w, h))) = mac_winops::ax_window_frame(pid, &title) {
        let eps = config::PLACE_EPS;
        if !(crate::tests::helpers::approx(x, ex0, eps)
            && crate::tests::helpers::approx(y, ey0, eps)
            && crate::tests::helpers::approx(w, ew0, eps)
            && h >= eh0 - eps)
        {
            return Err(Error::InvalidState(format!(
                "initial placement not anchored (ex x={:.1} y={:.1} w={:.1} h>={:.1}; got x={:.1} y={:.1} w={:.1} h={:.1})",
                ex0, ey0, ew0, eh0, x, y, w, h
            )));
        }
    }

    // Find the window id and request move right by one cell in same 4x4 grid
    let id = geom::find_window_id(
        pid,
        &title,
        config::DEFAULT_TIMEOUT_MS,
        config::PLACE_POLL_MS,
    )
    .ok_or_else(|| Error::InvalidState("failed to resolve WindowId for helper".into()))?;
    mac_winops::request_place_move_grid(id, 4, 4, mac_winops::MoveDir::Right)
        .map_err(|e| Error::SpawnFailed(format!("request_place_move_grid failed: {}", e)))?;
    // Drain the main ops queue on the main thread to apply the move.
    mac_winops::drain_main_ops();

    // Expected anchors after moving right: x changes to col=1; bottom flush; width equals cell width; height >= cell height.
    let (ex1, ey1, ew1, eh1) = geom::cell_rect(vf, 4, 4, 1, 0);
    let eps = config::PLACE_EPS;
    let deadline =
        std::time::Instant::now() + std::time::Duration::from_millis(config::PLACE_STEP_TIMEOUT_MS);
    let mut ok = false;
    while std::time::Instant::now() < deadline {
        if let Some(((x, y), (w, h))) = mac_winops::ax_window_frame(pid, &title)
            && crate::tests::helpers::approx(x, ex1, eps)
            && crate::tests::helpers::approx(y, ey1, eps)
            && crate::tests::helpers::approx(w, ew1, eps)
            && h >= eh1 - eps
        {
            ok = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(config::PLACE_POLL_MS));
    }
    if !ok {
        let actual =
            mac_winops::ax_window_frame(helper.pid, &title).map(|((x, y), (w, h))| (x, y, w, h));
        return Err(Error::InvalidState(match actual {
            Some((x, y, w, h)) => format!(
                "place-move-min mismatch (expected col=1 anchors; ex x={:.1} y={:.1} w={:.1} h>={:.1}; got x={:.1} y={:.1} w={:.1} h={:.1})",
                ex1, ey1, ew1, eh1, x, y, w, h
            ),
            None => "place-move-min mismatch (frame unavailable)".into(),
        }));
    }

    let _ = helper.kill_and_wait();
    Ok(())
}
