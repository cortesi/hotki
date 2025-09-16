//! Flexible placement smoketest used to exercise Stage-3/8 behaviors.
use std::{
    cmp, process,
    time::{SystemTime, UNIX_EPOCH},
};

use mac_winops::{FallbackTrigger, PlaceAttemptOptions};

use crate::{
    config,
    error::{Error, Result},
    helper_window::{ensure_frontmost, spawn_helper_visible, wait_for_frontmost_title},
    tests::fixtures::{self, Rect},
};

/// Run the flexible placement smoketest with configurable grid/cell and options.
pub fn run_place_flex(
    cols: u32,
    rows: u32,
    col: u32,
    row: u32,
    force_size_pos: bool,
    pos_first_only: bool,
    force_shrink_move_grow: bool,
) -> Result<()> {
    // Create unique helper title
    let now_pre = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let title = format!("hotki smoketest: place-flex {}-{}", process::id(), now_pre);

    // Spawn helper and wait until visible
    let lifetime = config::DEFAULTS.timeout_ms + config::HELPER_WINDOW.extra_time_ms;
    let mut helper = spawn_helper_visible(
        &title,
        lifetime,
        cmp::min(
            config::DEFAULTS.timeout_ms,
            config::HIDE.first_window_max_ms,
        ),
        config::PLACE.poll_ms,
        "FLEX",
    )?;

    // Bring to front to ensure mac-winops targets the correct focused window
    ensure_frontmost(helper.pid, &title, 3, 50);
    let _ = wait_for_frontmost_title(&title, config::WAITS.first_window_ms);

    // Compute expected rect from screen VF containing current AX position
    let vf = fixtures::resolve_vf_for_window(
        helper.pid,
        &title,
        config::DEFAULTS.timeout_ms,
        config::PLACE.poll_ms,
    )
    .ok_or_else(|| Error::InvalidState("Failed to resolve screen visibleFrame".into()))?;
    let expected = fixtures::cell_rect(vf, cols, rows, col, row);

    // Build attempt options for placement
    let mut opts = PlaceAttemptOptions::default()
        .with_force_second_attempt(force_size_pos || force_shrink_move_grow)
        .with_pos_first_only(pos_first_only);
    if force_shrink_move_grow {
        opts = opts.with_fallback_hook(|invocation| {
            matches!(
                invocation.trigger,
                FallbackTrigger::Forced | FallbackTrigger::Final
            )
        });
    }

    // Call mac-winops directly to place the focused window
    mac_winops::place_grid_focused_opts(helper.pid, cols, rows, col, row, opts)
        .map_err(|e| Error::InvalidState(format!("place_grid_focused failed: {}", e)))?;

    // Verify expected frame
    let ok = fixtures::wait_for_expected_frame(
        helper.pid,
        &title,
        expected,
        config::PLACE.eps,
        config::PLACE.step_timeout_ms,
        config::PLACE.poll_ms,
    );
    if !ok {
        let actual = mac_winops::ax_window_frame(helper.pid, &title)
            .map(|((ax, ay), (aw, ah))| Rect::new(ax, ay, aw, ah));
        return Err(Error::SpawnFailed(match actual {
            Some(actual) => format!(
                "place-flex mismatch (expected x={:.1} y={:.1} w={:.1} h={:.1}; actual x={:.1} y={:.1} w={:.1} h={:.1})",
                expected.x,
                expected.y,
                expected.w,
                expected.h,
                actual.x,
                actual.y,
                actual.w,
                actual.h
            ),
            None => format!(
                "place-flex mismatch (expected x={:.1} y={:.1} w={:.1} h={:.1}; actual frame unavailable)",
                expected.x, expected.y, expected.w, expected.h
            ),
        }));
    }

    if let Err(_e) = helper.kill_and_wait() {}
    Ok(())
}
