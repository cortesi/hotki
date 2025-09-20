//! Placement smoketest with resize increments.
//! Simulates terminal-style size rounding and verifies anchored edges.

use std::{
    cmp, thread,
    time::{Duration, Instant},
};

use hotki_world_ids::WorldWindowId;

use crate::{
    config,
    error::{Error, Result},
    helper_window::{HelperWindowBuilder, ManagedChild, ensure_frontmost, wait_for_window_visible},
    test_runner::{TestConfig, TestRunner},
    tests::fixtures::{self, Rect},
    world,
};

/// Check whether the window frame anchors to selected edges within tolerance.
fn verify_anchored(
    pid: i32,
    title: &str,
    expected: Rect,
    anchor_left: bool,
    anchor_right: bool,
    anchor_bottom: bool,
    anchor_top: bool,
) -> bool {
    let right = expected.x + expected.w;
    let top = expected.y + expected.h;
    let eps = config::PLACE.eps;
    let deadline = Instant::now() + Duration::from_millis(config::PLACE.step_timeout_ms);
    while Instant::now() < deadline {
        if let Some(((x, y), (w, h))) = mac_winops::ax_window_frame(pid, title) {
            let mut ok = true;
            if anchor_left {
                ok &= fixtures::approx(x, expected.x, eps);
            }
            if anchor_right {
                ok &= fixtures::approx(x + w, right, eps);
            }
            if anchor_bottom {
                ok &= fixtures::approx(y, expected.y, eps);
            }
            if anchor_top {
                ok &= fixtures::approx(y + h, top, eps);
            }
            if ok {
                return true;
            }
        }
        thread::sleep(Duration::from_millis(config::PLACE.poll_ms));
    }
    false
}

/// Run the increments placement smoketest with a 2×2 and 3×1 scenario.
pub fn run_place_increments_test(timeout_ms: u64, with_logs: bool) -> Result<()> {
    let helper_title = config::test_title("place-increments");

    // Minimal hotki config so backend is up; direct placement flows through hotki-world.
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
                .saturating_add(config::HELPER_WINDOW.extra_time_ms);
            let mut helper: ManagedChild = HelperWindowBuilder::new(title.clone())
                .with_time_ms(helper_time)
                .with_label_text("INC")
                .with_step_size(9.0, 18.0)
                .spawn_inherit_io()?;

            // Wait for visibility
            if !wait_for_window_visible(
                helper.pid,
                &title,
                cmp::min(ctx.config.timeout_ms, config::HIDE.first_window_max_ms),
                config::PLACE.poll_ms,
            ) {
                return Err(Error::InvalidState("helper window not visible".into()));
            }

            // Ensure frontmost
            ensure_frontmost(helper.pid, &title, 5, config::INPUT_DELAYS.retry_delay_ms);

            let window_id =
                fixtures::find_window_id(helper.pid, &title, 2000, config::PLACE.poll_ms)
                    .ok_or_else(|| {
                        Error::InvalidState("Failed to resolve helper CGWindowId".into())
                    })?;
            let target = WorldWindowId::new(helper.pid, window_id);

            // Case A: 2x2 bottom-right cell — expect right and TOP edges flush
            {
                let cols = 2u32;
                let rows = 2u32;
                let col = 1u32;
                let row = 1u32; // last row => top anchored
                let expected = {
                    let ((ax, ay), _) = mac_winops::ax_window_frame(helper.pid, &title)
                        .ok_or_else(|| Error::InvalidState("No AX frame for helper".into()))?;
                    let vf = fixtures::visible_frame_containing_point(ax, ay).ok_or_else(|| {
                        Error::InvalidState("Failed to resolve visibleFrame".into())
                    })?;
                    fixtures::cell_rect(vf, cols, rows, col, row)
                };
                world::place_window(target, cols, rows, col, row, None)?;
                let ok = verify_anchored(helper.pid, &title, expected, false, true, false, true);
                if !ok {
                    let actual = mac_winops::ax_window_frame(helper.pid, &title)
                        .map(|((x, y), (w, h))| Rect::new(x, y, w, h));
                    let case = format!("place_increments[case=A,col={},row={}]", col, row);
                    let msg = fixtures::frame_failure_line::<&str>(
                        &case,
                        expected,
                        actual,
                        config::PLACE.eps,
                        &[],
                    );
                    return Err(Error::InvalidState(msg));
                }
            }

            // Case B: 3x1 middle cell — expect LEFT and BOTTOM edges flush (matches WezTerm trace)
            {
                let cols = 3u32;
                let rows = 1u32;
                let col = 1u32; // middle
                let row = 0u32;
                let expected = {
                    let ((ax, ay), _) = mac_winops::ax_window_frame(helper.pid, &title)
                        .ok_or_else(|| Error::InvalidState("No AX frame for helper".into()))?;
                    let vf = fixtures::visible_frame_containing_point(ax, ay).ok_or_else(|| {
                        Error::InvalidState("Failed to resolve visibleFrame".into())
                    })?;
                    fixtures::cell_rect(vf, cols, rows, col, row)
                };
                world::place_window(target, cols, rows, col, row, None)?;
                let ok = verify_anchored(helper.pid, &title, expected, true, false, true, false);
                if !ok {
                    let actual = mac_winops::ax_window_frame(helper.pid, &title)
                        .map(|((x, y), (w, h))| Rect::new(x, y, w, h));
                    let case = format!("place_increments[case=B,col={},row={}]", col, row);
                    let msg = fixtures::frame_failure_line::<&str>(
                        &case,
                        expected,
                        actual,
                        config::PLACE.eps,
                        &[],
                    );
                    return Err(Error::InvalidState(msg));
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
