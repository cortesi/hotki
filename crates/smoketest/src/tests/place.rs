//! Grid placement smoketest.
//!
//! What this verifies
//! - Placing a window into every cell of an `cols × rows` grid positions it to
//!   the exact cell rectangle derived from the screen’s AppKit visible frame.
//! - The test computes the expected rectangle for each cell and compares the
//!   observed AX frame against it.
//!
//! Acceptance criteria
//! - For every `(col,row)` cell, after sending the bound key, the helper
//!   window’s `(x, y, w, h)` matches the expected cell rectangle within
//!   `PLACE_EPS` tolerance and before `PLACE_STEP_TIMEOUT_MS` expires.
//! - If the helper CGWindowId cannot be found, the screen’s visible frame
//!   cannot be resolved, or any cell fails to match within tolerance/time, the
//!   test fails with a detailed mismatch error (expected vs. actual).
//!
//! Notes
//! - The HUD is hidden; a `g` binding raises the helper before each placement
//!   to ensure the correct target pid.

use super::{
    geom,
    helpers::{HelperWindow, wait_for_frontmost_title},
};
use crate::{
    config,
    error::{Error, Result},
    test_runner::{TestConfig, TestRunner},
    ui_interaction::send_key,
};

// Geometry helpers are provided by `tests::geom`.

/// Run grid placement test across all cells of the default grid.
pub fn run_place_test(timeout_ms: u64, with_logs: bool) -> Result<()> {
    // Bind all actions directly at the top level (no nested modes, HUD hidden).
    // Keys:
    //   g → raise helper by title (noexit)
    //   1..N (and letters thereafter) → place into each grid cell (row-major)
    let cols = config::PLACE_COLS;
    let rows = config::PLACE_ROWS;
    let mut entries = String::new();
    let mut key_for_cell: Vec<(u32, u32, char)> = Vec::new();
    let mut keycode = 1usize;
    for row in 0..rows {
        for col in 0..cols {
            let ch = if keycode <= 9 {
                char::from_digit(keycode as u32, 10).unwrap()
            } else {
                // After '9', continue with letters a, b, c ...
                (b'a' + (keycode as u8 - 10)) as char
            };
            entries.push_str(&format!(
                "            (\"{}\", \"({}, {})\", place(grid({}, {}), at({}, {}))),\n",
                ch, col, row, cols, rows, col, row
            ));
            key_for_cell.push((col, row, ch));
            keycode += 1;
        }
    }
    // Precompute helper title and embed a raise binding that targets it.
    let helper_title = config::test_title("place");
    let ron_config: String = format!(
        "(\n    keys: [\n        (\"g\", \"raise\", raise(title: \"{}\"), (noexit: true)),\n{}    ],\n    style: (hud: (mode: hide)),\n    server: (exit_if_no_clients: true),\n)\n",
        helper_title, entries
    );
    let config = TestConfig::new(timeout_ms)
        .with_logs(with_logs)
        // HUD remains hidden; bind top-level keys directly
        .with_temp_config(ron_config);

    TestRunner::new("place_test", config)
        .with_setup(|ctx| {
            ctx.launch_hotki()?;
            // Gate on top-level bindings being present
            let _ = ctx.ensure_rpc_ready(&["g", "1"]);
            Ok(())
        })
        .with_execute(move |ctx| {
            // Spawn helper window using the title embedded in the config
            let title = helper_title.clone();
            let helper_time = ctx
                .config
                .timeout_ms
                .saturating_add(config::HELPER_WINDOW_EXTRA_TIME_MS);
            let mut helper = HelperWindow::spawn_frontmost(
                &title,
                helper_time,
                std::cmp::min(ctx.config.timeout_ms, config::HIDE_FIRST_WINDOW_MAX_MS),
                config::PLACE_POLL_MS,
                "P",
            )?;

            // Resolve CG window id
            let _wid = geom::find_window_id(helper.pid, &title, 2000, config::PLACE_POLL_MS)
                .ok_or_else(|| Error::InvalidState("Failed to resolve helper CGWindowId".into()))?;

            // Ensure helper is frontmost via backend raise binding; actively wait for it
            send_key("g");
            let _ = wait_for_frontmost_title(&title, config::WAIT_FIRST_WINDOW_MS);

            // Iterate all grid cells in row-major order (top-left is (0,0))
            for (col, row, key) in key_for_cell {
                    // Re-raise helper to ensure engine targets the right pid and wait until frontmost
                    send_key("g");
                    let _ = wait_for_frontmost_title(&title, config::WAIT_FIRST_WINDOW_MS);
                    // Recompute visible frame based on current AX position (matches backend logic)
                    let vf = geom::resolve_vf_for_window(
                        helper.pid,
                        &title,
                        config::DEFAULT_TIMEOUT_MS,
                        config::PLACE_POLL_MS,
                    )
                    .ok_or_else(|| Error::InvalidState("Failed to resolve screen visibleFrame".into()))?;

                    // Compute expected cell rect
                    let (ex, ey, ew, eh) = geom::cell_rect(vf, cols, rows, col, row);

                    // Drive backend: send the key for this cell directly (no nested modes)
                    let key_str = key.to_string();
                    send_key(&key_str);

                    // Wait for expected frame within tolerance
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
                                "placement mismatch at col={}, row={} (expected x={:.1} y={:.1} w={:.1} h={:.1}; actual x={:.1} y={:.1} w={:.1} h={:.1})",
                                col, row, ex, ey, ew, eh, ax, ay, aw, ah
                            ),
                            None => format!(
                                "placement mismatch at col={}, row={} (expected x={:.1} y={:.1} w={:.1} h={:.1}; actual frame unavailable)",
                                col, row, ex, ey, ew, eh
                            ),
                        }));
                    }
                }

            // Kill helper explicitly to exercise teardown
            if let Err(_e) = helper.kill_and_wait() {}
            Ok(())
        })
        .with_teardown(|ctx, _| {
            ctx.shutdown();
            Ok(())
        })
        .run()
}
