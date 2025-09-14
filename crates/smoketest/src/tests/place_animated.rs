//! Animated placement smoketest: helper tweens to the requested frame over ~120ms.
//! Verifies that engine polling/settle logic converges within the default budget.

use crate::{
    config,
    error::{Error, Result},
    process::{HelperWindowBuilder, ManagedChild},
    tests::geom,
};

pub fn run_place_animated_test(timeout_ms: u64, with_logs: bool) -> Result<()> {
    let cols = 2u32;
    let rows = 2u32;
    let col = 1u32; // BR
    let row = 1u32;
    let helper_title = crate::config::test_title("place-animated");

    // Minimal hotki config so backend is up; direct mac-winops call drives placement.
    let ron_config: String =
        "(keys: [], style: (hud: (mode: hide)), server: (exit_if_no_clients: true))\n".into();

    let cfg = crate::test_runner::TestConfig::new(timeout_ms)
        .with_logs(with_logs)
        .with_temp_config(ron_config);

    crate::test_runner::TestRunner::new("place_animated", cfg)
        .with_setup(|ctx| {
            ctx.launch_hotki()?;
            let _ = ctx.ensure_rpc_ready(&[]);
            Ok(())
        })
        .with_execute(move |ctx| {
            let title = helper_title.clone();
            let helper_time = ctx
                .config
                .timeout_ms
                .saturating_add(config::HELPER_WINDOW_EXTRA_TIME_MS);
            let mut helper: ManagedChild = HelperWindowBuilder::new(title.clone())
                .with_time_ms(helper_time)
                .with_label_text("A")
                .with_tween_ms(120)
                .with_delay_apply_grid(50, cols, rows, col, row)
                .spawn_inherit_io()?;

            // Wait for visibility
            if !crate::tests::helpers::wait_for_window_visible(
                helper.pid,
                &title,
                std::cmp::min(ctx.config.timeout_ms, config::HIDE_FIRST_WINDOW_MAX_MS),
                config::PLACE_POLL_MS,
            ) {
                return Err(Error::InvalidState("helper window not visible".into()));
            }

            // Resolve window id and ensure frontmost
            let _ = geom::find_window_id(helper.pid, &title, 2000, config::PLACE_POLL_MS)
                .ok_or_else(|| Error::InvalidState("Failed to resolve helper CGWindowId".into()))?;
            crate::tests::helpers::ensure_frontmost(
                helper.pid,
                &title,
                5,
                config::RETRY_DELAY_MS,
            );

            // Compute expected rect on the helper's current screen from CG, then
            // wait until CG bounds approximately match it (no AX dependency).
            let (expected_x, expected_y, expected_w, expected_h) = {
                let start = mac_winops::list_windows()
                    .into_iter()
                    .find(|w| w.pid == helper.pid && w.title == title)
                    .and_then(|w| w.pos)
                    .ok_or_else(|| Error::InvalidState("No CG bounds for helper".into()))?;
                let vf = crate::tests::geom::visible_frame_containing_point(
                    start.x as f64,
                    start.y as f64,
                )
                .ok_or_else(|| Error::InvalidState("Failed to resolve visibleFrame".into()))?;
                let (ex, ey, ew, eh) = crate::tests::geom::cell_rect(vf, cols, rows, col, row);
                (ex, ey, ew, eh)
            };
            let cg_ok = {
                let deadline = std::time::Instant::now()
                    + std::time::Duration::from_millis(config::PLACE_STEP_TIMEOUT_MS);
                let eps = 2.0_f64;
                let mut ok = false;
                while std::time::Instant::now() < deadline {
                    if let Some(pos) = mac_winops::list_windows()
                        .into_iter()
                        .find(|w| w.pid == helper.pid && w.title == title)
                        .and_then(|w| w.pos)
                    {
                        let (x, y, w, h) =
                            (pos.x as f64, pos.y as f64, pos.width as f64, pos.height as f64);
                        if crate::tests::helpers::approx(x, expected_x, eps)
                            && crate::tests::helpers::approx(y, expected_y, eps)
                            && crate::tests::helpers::approx(w, expected_w, eps)
                            && crate::tests::helpers::approx(h, expected_h, eps)
                        {
                            ok = true;
                            break;
                        }
                    }
                    std::thread::sleep(std::time::Duration::from_millis(config::PLACE_POLL_MS));
                }
                ok
            };
            if !cg_ok {
                return Err(Error::SpawnFailed(format!(
                    "animated window did not reach expected CG rect (expected x={:.1} y={:.1} w={:.1} h={:.1})",
                    expected_x, expected_y, expected_w, expected_h
                )));
            }

            let _ = helper.kill_and_wait();
            Ok(())
        })
        .with_teardown(|ctx, _| {
            ctx.shutdown();
            Ok(())
        })
        .run()
}
