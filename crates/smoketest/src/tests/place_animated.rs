//! Animated placement smoketest: helper tweens to the requested frame over ~120ms.
//! Verifies that engine polling/settle logic converges within the default budget.

use std::{
    cmp, thread,
    time::{Duration, Instant},
};

use crate::{
    config,
    error::{Error, Result},
    helper_window::{HelperWindowBuilder, ManagedChild, ensure_frontmost, wait_for_window_visible},
    test_runner::{TestConfig, TestRunner},
    tests::fixtures,
    world,
};

/// Run the animated placement smoketest with a small 2×2 grid.
pub fn run_place_animated_test(timeout_ms: u64, with_logs: bool) -> Result<()> {
    let cols = 2u32;
    let rows = 2u32;
    let col = 1u32; // BR
    let row = 1u32;
    let helper_title = config::test_title("place-animated");

    // Minimal hotki config so backend is up; placements go through hotki-world.
    let ron_config: String =
        "(keys: [], style: (hud: (mode: hide)), server: (exit_if_no_clients: true))\n".into();

    let cfg = TestConfig::new(timeout_ms)
        .with_logs(with_logs)
        .with_temp_config(ron_config);

    TestRunner::new("place_animated", cfg)
        .with_setup(|ctx| {
            ctx.launch_hotki()?;
            ctx.ensure_rpc_ready(&[])?;
            Ok(())
        })
        .with_execute(move |ctx| {
            let title = helper_title.clone();
            let helper_time = ctx
                .config
                .timeout_ms
                .saturating_add(config::HELPER_WINDOW.extra_time_ms);
            let mut helper: ManagedChild = HelperWindowBuilder::new(title.clone())
                .with_time_ms(helper_time)
                .with_label_text("A")
                .with_tween_ms(120)
                .with_delay_apply_grid(50, cols, rows, col, row)
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

            // Resolve window id and ensure frontmost
            let _ =
                fixtures::find_window_id(helper.pid, &title, 2000, config::PLACE.poll_ms)
                    .ok_or_else(|| Error::InvalidState("Failed to resolve helper CGWindowId".into()))?;
            ensure_frontmost(
                helper.pid,
                &title,
                5,
                config::INPUT_DELAYS.retry_delay_ms,
            );

            // Compute expected rect on the helper's current screen from CG, then
            // wait until CG bounds approximately match it (no AX dependency).
            let expected = {
                let start = world::list_windows_or_empty()
                    .into_iter()
                    .find(|w| w.pid == helper.pid && w.title == title)
                    .and_then(|w| w.pos)
                    .ok_or_else(|| Error::InvalidState("No world bounds for helper".into()))?;
                let vf = fixtures::visible_frame_containing_point(
                    start.x as f64,
                    start.y as f64,
                )
                .ok_or_else(|| Error::InvalidState("Failed to resolve visibleFrame".into()))?;
                fixtures::cell_rect(vf, cols, rows, col, row)
            };
            let cg_ok = {
                let deadline =
                    Instant::now() + Duration::from_millis(config::PLACE.step_timeout_ms);
                let eps = config::PLACE.eps;
                let mut ok = false;
                while Instant::now() < deadline {
                    if let Some(pos) = world::list_windows_or_empty()
                        .into_iter()
                        .find(|w| w.pid == helper.pid && w.title == title)
                        .and_then(|w| w.pos)
                    {
                        let (x, y, w, h) =
                            (pos.x as f64, pos.y as f64, pos.width as f64, pos.height as f64);
                        if fixtures::approx(x, expected.x, eps)
                            && fixtures::approx(y, expected.y, eps)
                            && fixtures::approx(w, expected.w, eps)
                            && fixtures::approx(h, expected.h, eps)
                        {
                            ok = true;
                            break;
                        }
                    }
                    thread::sleep(Duration::from_millis(config::PLACE.poll_ms));
                }
                ok
            };
            if !cg_ok {
                return Err(Error::SpawnFailed(format!(
                    "animated window did not reach expected CG rect (expected x={:.1} y={:.1} w={:.1} h={:.1})",
                    expected.x, expected.y, expected.w, expected.h
                )));
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
