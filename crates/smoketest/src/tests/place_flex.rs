//! Flexible placement smoketest used to exercise Stage-3/8 behaviors.
use std::{
    cmp, process,
    time::{SystemTime, UNIX_EPOCH},
};

use mac_winops::PlaceAttemptOptions;

use crate::{
    config,
    error::{Error, Result},
    tests::{
        geom,
        helpers::{ensure_frontmost, spawn_helper_visible, wait_for_frontmost_title},
    },
};

// Geometry helpers moved to `tests::geom`.

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
    let lifetime = config::DEFAULT_TIMEOUT_MS + config::HELPER_WINDOW_EXTRA_TIME_MS;
    let mut helper = spawn_helper_visible(
        &title,
        lifetime,
        cmp::min(config::DEFAULT_TIMEOUT_MS, config::HIDE_FIRST_WINDOW_MAX_MS),
        config::PLACE_POLL_MS,
        "FLEX",
    )?;

    // Bring to front to ensure mac-winops targets the correct focused window
    ensure_frontmost(helper.pid, &title, 3, 50);
    let _ = wait_for_frontmost_title(&title, config::WAIT_FIRST_WINDOW_MS);

    // Compute expected rect from screen VF containing current AX position
    let (vf_x, vf_y, vf_w, vf_h) = geom::resolve_vf_for_window(
        helper.pid,
        &title,
        config::DEFAULT_TIMEOUT_MS,
        config::PLACE_POLL_MS,
    )
    .ok_or_else(|| Error::InvalidState("Failed to resolve screen visibleFrame".into()))?;
    let (ex, ey, ew, eh) = geom::cell_rect((vf_x, vf_y, vf_w, vf_h), cols, rows, col, row);

    // Build attempt options for placement
    let opts = PlaceAttemptOptions {
        force_second_attempt: force_size_pos || force_shrink_move_grow,
        pos_first_only,
        force_shrink_move_grow,
    };

    // Call mac-winops directly to place the focused window
    mac_winops::place_grid_focused_opts(helper.pid, cols, rows, col, row, opts)
        .map_err(|e| Error::InvalidState(format!("place_grid_focused failed: {}", e)))?;

    // Verify expected frame
    let ok = geom::wait_for_expected_frame(
        helper.pid,
        &title,
        (ex, ey, ew, eh),
        config::PLACE_EPS,
        config::PLACE_STEP_TIMEOUT_MS,
        config::PLACE_POLL_MS,
    );
    if !ok {
        let actual = mac_winops::ax_window_frame(helper.pid, &title)
            .map(|((ax, ay), (aw, ah))| (ax, ay, aw, ah));
        return Err(Error::SpawnFailed(match actual {
            Some((ax, ay, aw, ah)) => format!(
                "place-flex mismatch (expected x={:.1} y={:.1} w={:.1} h={:.1}; actual x={:.1} y={:.1} w={:.1} h={:.1})",
                ex, ey, ew, eh, ax, ay, aw, ah
            ),
            None => format!(
                "place-flex mismatch (expected x={:.1} y={:.1} w={:.1} h={:.1}; actual frame unavailable)",
                ex, ey, ew, eh
            ),
        }));
    }

    if let Err(_e) = helper.kill_and_wait() {}
    Ok(())
}
