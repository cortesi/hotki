//! Placement smoketest with resize increments.
//! Simulates terminal-style size rounding and verifies anchored edges.

use std::{
    cmp, thread,
    time::{Duration, Instant},
};

use crate::{
    config,
    error::{Error, Result},
    helper_window::{HelperWindowBuilder, ManagedChild, ensure_frontmost, wait_for_window_visible},
    test_runner::{TestConfig, TestRunner},
    tests::{geom, helpers::approx},
};

/// Check whether the window frame anchors to selected edges within tolerance.
fn verify_anchored(
    pid: i32,
    title: &str,
    expected: (f64, f64, f64, f64),
    anchor_left: bool,
    anchor_right: bool,
    anchor_bottom: bool,
    anchor_top: bool,
) -> bool {
    let (ex, ey, ew, eh) = expected;
    let right = ex + ew;
    let top = ey + eh;
    let eps = config::PLACE_EPS;
    let deadline = Instant::now() + Duration::from_millis(config::PLACE_STEP_TIMEOUT_MS);
    while Instant::now() < deadline {
        if let Some(((x, y), (w, h))) = mac_winops::ax_window_frame(pid, title) {
            let mut ok = true;
            if anchor_left {
                ok &= approx(x, ex, eps);
            }
            if anchor_right {
                ok &= approx(x + w, right, eps);
            }
            if anchor_bottom {
                ok &= approx(y, ey, eps);
            }
            if anchor_top {
                ok &= approx(y + h, top, eps);
            }
            if ok {
                return true;
            }
        }
        thread::sleep(Duration::from_millis(config::PLACE_POLL_MS));
    }
    false
}

/// Run the increments placement smoketest with a 2×2 and 3×1 scenario.
pub fn run_place_increments_test(timeout_ms: u64, with_logs: bool) -> Result<()> {
    let helper_title = config::test_title("place-increments");

    // Minimal hotki config so backend is up; direct mac-winops call drives placement.
    let ron_config: String =
        "(keys: [], style: (hud: (mode: hide)), server: (exit_if_no_clients: true))\n".into();

    let cfg = TestConfig::new(timeout_ms)
        .with_logs(with_logs)
        .with_temp_config(ron_config);

    TestRunner::new("place_increments", cfg)
        .with_setup(|ctx| {
            ctx.launch_hotki()?;
            ctx.ensure_rpc_ready(&[])?;
            Ok(())
        })
        .with_execute(move |ctx| {
            // Spawn helper with step-size rounding (approx 9x18 typical terminal cell size)
            let title = helper_title;
            let helper_time = ctx
                .config
                .timeout_ms
                .saturating_add(config::HELPER_WINDOW_EXTRA_TIME_MS);
            let mut helper: ManagedChild = HelperWindowBuilder::new(title.clone())
                .with_time_ms(helper_time)
                .with_label_text("INC")
                .with_step_size(9.0, 18.0)
                .spawn_inherit_io()?;

            // Wait for visibility
            if !wait_for_window_visible(
                helper.pid,
                &title,
                cmp::min(ctx.config.timeout_ms, config::HIDE_FIRST_WINDOW_MAX_MS),
                config::PLACE_POLL_MS,
            ) {
                return Err(Error::InvalidState("helper window not visible".into()));
            }

            // Ensure frontmost
            ensure_frontmost(
                helper.pid,
                &title,
                5,
                config::RETRY_DELAY_MS,
            );

            // Case A: 2x2 bottom-right cell — expect right and TOP edges flush
            {
                let cols = 2u32;
                let rows = 2u32;
                let col = 1u32;
                let row = 1u32; // last row => top anchored
                let (ex, ey, ew, eh) = {
                    let ((ax, ay), _) = mac_winops::ax_window_frame(helper.pid, &title)
                        .ok_or_else(|| Error::InvalidState("No AX frame for helper".into()))?;
                    let vf = geom::visible_frame_containing_point(ax, ay)
                        .ok_or_else(|| Error::InvalidState("Failed to resolve visibleFrame".into()))?;
                    geom::cell_rect(vf, cols, rows, col, row)
                };
                mac_winops::place_grid_focused(helper.pid, cols, rows, col, row)
                    .map_err(|e| Error::SpawnFailed(format!(
                        "place_grid_focused failed (2x2 BR): {}",
                        e
                    )))?;
                let ok = verify_anchored(helper.pid, &title, (ex, ey, ew, eh), false, true, false, true);
                if !ok {
                    let actual = mac_winops::ax_window_frame(helper.pid, &title)
                        .map(|((x, y), (w, h))| (x, y, w, h));
                    return Err(Error::SpawnFailed(match actual {
                        Some((x, y, w, h)) => format!(
                            "increments A not anchored (expect right+top flush; ex={:.1},{:.1},{:.1},{:.1}; got x={:.1} y={:.1} w={:.1} h={:.1})",
                            ex, ey, ew, eh, x, y, w, h
                        ),
                        None => "increments A not anchored (frame unavailable)".into(),
                    }));
                }
            }

            // Case B: 3x1 middle cell — expect LEFT and BOTTOM edges flush (matches WezTerm trace)
            {
                let cols = 3u32;
                let rows = 1u32;
                let col = 1u32; // middle
                let row = 0u32;
                let (ex, ey, ew, eh) = {
                    let ((ax, ay), _) = mac_winops::ax_window_frame(helper.pid, &title)
                        .ok_or_else(|| Error::InvalidState("No AX frame for helper".into()))?;
                    let vf = geom::visible_frame_containing_point(ax, ay)
                        .ok_or_else(|| Error::InvalidState("Failed to resolve visibleFrame".into()))?;
                    geom::cell_rect(vf, cols, rows, col, row)
                };
                mac_winops::place_grid_focused(helper.pid, cols, rows, col, row)
                    .map_err(|e| Error::SpawnFailed(format!(
                        "place_grid_focused failed (3x1 mid): {}",
                        e
                    )))?;
                let ok = verify_anchored(helper.pid, &title, (ex, ey, ew, eh), true, false, true, false);
                if !ok {
                    let actual = mac_winops::ax_window_frame(helper.pid, &title)
                        .map(|((x, y), (w, h))| (x, y, w, h));
                    return Err(Error::SpawnFailed(match actual {
                        Some((x, y, w, h)) => format!(
                            "increments B not anchored (expect left+bottom flush; ex={:.1},{:.1},{:.1},{:.1}; got x={:.1} y={:.1} w={:.1} h={:.1})",
                            ex, ey, ew, eh, x, y, w, h
                        ),
                        None => "increments B not anchored (frame unavailable)".into(),
                    }));
                }
            }

            if let Err(_e) = helper.kill_and_wait() {}
            Ok(())
        })
        .with_teardown(|ctx, _| {
            ctx.shutdown();
            Ok(())
        })
        .run()
}
