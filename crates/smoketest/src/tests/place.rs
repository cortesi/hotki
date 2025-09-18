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
//!   `config::PLACE.eps` tolerance and before `config::PLACE.step_timeout_ms`
//!   expires.
//! - If the helper CGWindowId cannot be found, the screen’s visible frame
//!   cannot be resolved, or any cell fails to match within tolerance/time, the
//!   test fails with a detailed mismatch error (expected vs. actual).
//!
//! Notes
//! - The HUD is hidden; a `g` binding raises the helper before each placement
//!   to ensure the correct target pid.

use std::cmp;

use super::fixtures::{self, Rect};
use crate::{
    config,
    error::{Error, Result},
    helper_window::{HelperWindow, wait_for_frontmost_title},
    test_runner::{TestConfig, TestRunner},
    world,
};
use hotki_world_ids::WorldWindowId;

// Geometry helpers are provided by `tests::fixtures`.

/// Run grid placement test across all cells of the default grid.
pub fn run_place_test(timeout_ms: u64, with_logs: bool) -> Result<()> {
    // Minimal backend (HUD hidden); we drive placement via hotki-world so we
    // never resize a non-test window.
    let cols = config::PLACE.grid_cols;
    let rows = config::PLACE.grid_rows;
    let helper_title = config::test_title("place");
    let ron_config: String =
        "(keys: [], style: (hud: (mode: hide)), server: (exit_if_no_clients: true))\n".into();
    let config = TestConfig::new(timeout_ms)
        .with_logs(with_logs)
        // HUD remains hidden; bind top-level keys directly
        .with_temp_config(ron_config);

    TestRunner::new("place_test", config)
        .with_setup(|ctx| {
            ctx.launch_hotki()?;
            // No MRPC bindings needed; placement driven via hotki-world APIs.
            ctx.ensure_rpc_ready(&[])?;
            Ok(())
        })
        .with_execute(move |ctx| {
            // Spawn helper window using the title embedded in the config
            let title = helper_title.clone();
            let helper_time = ctx
                .config
                .timeout_ms
                .saturating_add(config::HELPER_WINDOW.extra_time_ms);
            let mut helper = HelperWindow::spawn_frontmost(
                &title,
                helper_time,
                cmp::min(ctx.config.timeout_ms, config::HIDE.first_window_max_ms),
                config::PLACE.poll_ms,
                "P",
            )?;

            // Resolve CG window id
            let window_id = fixtures::find_window_id(helper.pid, &title, 2000, config::PLACE.poll_ms)
                .ok_or_else(|| Error::InvalidState("Failed to resolve helper CGWindowId".into()))?;
            let target = WorldWindowId::new(helper.pid, window_id);

            // Ensure helper is frontmost to make AX resolution stable
            let _ = wait_for_frontmost_title(&title, config::WAITS.first_window_ms);

            // Iterate all grid cells in row-major order (top-left is (0,0)) and
            // drive placement through hotki-world for the helper PID.
            for row in 0..rows {
                for col in 0..cols {
                    // Resolve visible frame based on current AX position
                    let vf = fixtures::resolve_vf_for_window(
                        helper.pid,
                        &title,
                        config::DEFAULTS.timeout_ms,
                        config::PLACE.poll_ms,
                    )
                    .ok_or_else(|| Error::InvalidState("Failed to resolve screen visibleFrame".into()))?;

                    // Compute expected cell rect
                    let expected = fixtures::cell_rect(vf, cols, rows, col, row);

                    // Place only the focused window for the helper's PID
                    world::place_window(target, cols, rows, col, row, None)?;

                    // Wait for expected frame within tolerance
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
                                "placement mismatch at col={}, row={} (expected x={:.1} y={:.1} w={:.1} h={:.1}; actual x={:.1} y={:.1} w={:.1} h={:.1})",
                                col,
                                row,
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
                                "placement mismatch at col={}, row={} (expected x={:.1} y={:.1} w={:.1} h={:.1}; actual frame unavailable)",
                                col, row, expected.x, expected.y, expected.w, expected.h
                            ),
                        }));
                    }
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
