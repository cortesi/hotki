//! Async placement smoketest: helper delays applying window frame changes by ~50ms.
//! Verifies that engine polling/settle logic converges within the default budget.

// no direct time imports needed; use config::test_title

use crate::{
    config,
    error::{Error, Result},
    process::{HelperWindowBuilder, ManagedChild},
    tests::geom,
};

// Geometry helpers moved to `tests::geom`.

pub fn run_place_async_test(timeout_ms: u64, with_logs: bool) -> Result<()> {
    let cols = 2u32;
    let rows = 2u32;
    let col = 1u32; // BR
    let row = 1u32;
    let helper_title = crate::config::test_title("place-async");

    // Build a minimal hotki config so the backend is up (but we will call
    // mac-winops directly for placement to reduce orchestration flakiness).
    let ron_config: String =
        "(keys: [], style: (hud: (mode: hide)), server: (exit_if_no_clients: true))\n".into();

    let cfg = crate::test_runner::TestConfig::new(timeout_ms)
        .with_logs(with_logs)
        .with_temp_config(ron_config);

    crate::test_runner::TestRunner::new("place_async", cfg)
        .with_setup(|ctx| {
            ctx.launch_hotki()?;
            // Ensure the MRPC driver is initialized (no specific idents required here).
            let _ = ctx.ensure_rpc_ready(&[]);
            Ok(())
        })
        .with_execute(move |ctx| {
            // Spawn helper with delayed setFrame behavior
            let title = helper_title.clone();
            let helper_time = ctx
                .config
                .timeout_ms
                .saturating_add(config::HELPER_WINDOW_EXTRA_TIME_MS);
            // Compute expected grid rect now based on the main or first screen;
            // the helper spawns on the main screen by default.
            let mut helper: ManagedChild = HelperWindowBuilder::new(title.clone())
                .with_time_ms(helper_time)
                .with_label_text("A")
                .with_delay_apply_grid(50, cols, rows, col, row)
                .spawn()?;

            // Wait for visibility
            if !crate::tests::helpers::wait_for_window_visible(
                helper.pid,
                &title,
                std::cmp::min(ctx.config.timeout_ms, config::HIDE_FIRST_WINDOW_MAX_MS),
                config::PLACE_POLL_MS,
            ) {
                return Err(Error::InvalidState("helper window not visible".into()));
            }

            // Resolve window id and ensure frontmost by best-effort activation
            let _ = geom::find_window_id(helper.pid, &title, 2000, config::PLACE_POLL_MS)
                .ok_or_else(|| Error::InvalidState("Failed to resolve helper CGWindowId".into()))?;
            crate::tests::helpers::ensure_frontmost(
                helper.pid,
                &title,
                5,
                config::RETRY_DELAY_MS,
            );

            // Compute expected rect for (1,1) at current screen
            let (vf_x, vf_y, vf_w, vf_h) = geom::resolve_vf_for_window(
                helper.pid,
                &title,
                config::DEFAULT_TIMEOUT_MS,
                config::PLACE_POLL_MS,
            )
            .ok_or_else(|| Error::InvalidState("Failed to resolve screen visibleFrame".into()))?;
            let expected = geom::cell_rect((vf_x, vf_y, vf_w, vf_h), cols, rows, col, row);

            // Trigger placement directly via mac-winops (focused-for-pid)
            // This exercises the exact placement code-path while avoiding
            // orchestrator races.
            mac_winops::place_grid_focused(helper.pid, cols, rows, col, row)
                .map_err(|e| Error::SpawnFailed(format!("place_grid_focused failed: {}", e)))?;
            let ok = geom::wait_for_expected_frame(
                helper.pid,
                &title,
                expected,
                config::PLACE_EPS,
                config::PLACE_STEP_TIMEOUT_MS,
                config::PLACE_POLL_MS,
            );
            if !ok {
                let actual = mac_winops::ax_window_frame(helper.pid, &title)
                    .map(|((ax, ay), (aw, ah))| (ax, ay, aw, ah));
                return Err(Error::SpawnFailed(match actual {
                    Some((ax, ay, aw, ah)) => format!(
                        "placement mismatch (async) (expected x={:.1} y={:.1} w={:.1} h={:.1}; actual x={:.1} y={:.1} w={:.1} h={:.1})",
                        expected.0, expected.1, expected.2, expected.3, ax, ay, aw, ah
                    ),
                    None => format!(
                        "placement mismatch (async) (expected x={:.1} y={:.1} w={:.1} h={:.1}; actual frame unavailable)",
                        expected.0, expected.1, expected.2, expected.3
                    ),
                }));
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
