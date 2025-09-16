//! Placement normalization smoketests: exercise minimized/zoomed pre-states.

use std::{
    cmp, process, thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use super::fixtures;
use crate::{
    config,
    error::{Error, Result},
    helper_window::{ensure_frontmost, spawn_helper_with_options},
    test_runner::{TestConfig, TestRunner},
};

// Geometry and polling helpers are provided by `tests::fixtures`.

/// Drive a placement with initial minimized/zoomed state and verify normalization.
fn run_place_with_state(
    timeout_ms: u64,
    with_logs: bool,
    start_minimized: bool,
    start_zoomed: bool,
    label: String,
) -> Result<()> {
    let cols = config::PLACE.grid_cols;
    let rows = config::PLACE.grid_rows;
    let now_pre = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let helper_title = format!("hotki smoketest: place-state {}-{}", process::id(), now_pre);
    // Minimal backend; direct placement is driven via mac-winops and the helper PID.
    let ron_config: String =
        "(keys: [], style: (hud: (mode: hide)), server: (exit_if_no_clients: true))\n".into();
    let config = TestConfig::new(timeout_ms)
        .with_logs(with_logs)
        .with_temp_config(ron_config);

    TestRunner::new("place_state", config)
        .with_setup(|ctx| {
            ctx.launch_hotki()?;
            // No MRPC bindings are required; placement is driven via mac-winops APIs.
            ctx.ensure_rpc_ready(&[])?;
            Ok(())
        })
        .with_execute(move |ctx| {
            let title = helper_title.clone();
            let helper_time = ctx
                .config
                .timeout_ms
                .saturating_add(config::HELPER_WINDOW.extra_time_ms);
            let mut helper = spawn_helper_with_options(
                &title,
                helper_time,
                cmp::min(ctx.config.timeout_ms, config::HIDE.first_window_max_ms),
                config::PLACE.poll_ms,
                &label,
                start_minimized,
                start_zoomed,
            )?;

            // Best-effort: bring the helper frontmost for deterministic AXFocused/AXMain resolution
            ensure_frontmost(
                helper.pid,
                &title,
                3,
                config::INPUT_DELAYS.ui_action_delay_ms,
            );

            // If the helper started minimized, AX frame can lag after de-miniaturize.
            // Actively wait until an AX frame is available before issuing placement.
            let ready_deadline = Instant::now()
                + Duration::from_millis(cmp::min(
                    ctx.config.timeout_ms,
                    config::WAITS.first_window_ms,
                ));
            while Instant::now() < ready_deadline
                && mac_winops::ax_window_frame(helper.pid, &title).is_none()
            {
                thread::sleep(config::ms(config::PLACE.poll_ms));
            }

            let vf = if let Some(((ax, ay), _)) = mac_winops::ax_window_frame(helper.pid, &title) {
                fixtures::visible_frame_containing_point(ax, ay).ok_or_else(|| {
                    Error::InvalidState("Failed to resolve screen visibleFrame".into())
                })?
            } else if let Some(vf) = fixtures::resolve_vf_for_window(
                helper.pid,
                &title,
                config::DEFAULTS.timeout_ms,
                config::PLACE.poll_ms,
            ) {
                vf
            } else {
                return Err(Error::InvalidState(
                    "Failed to resolve screen visibleFrame".into(),
                ));
            };

            // Expected cell rect for (0,0)
            let expected = fixtures::cell_rect(vf, cols, rows, 0, 0);
            // Enforce constraint: place only the focused window for the helper's PID
            mac_winops::place_grid_focused(helper.pid, cols, rows, 0, 0)
                .map_err(|e| Error::InvalidState(format!("place_grid_focused failed: {}", e)))?;
            let ok = fixtures::wait_for_expected_frame(
                helper.pid,
                &title,
                expected,
                config::PLACE.eps,
                config::PLACE.step_timeout_ms,
                config::PLACE.poll_ms,
            );
            if !ok {
                return Err(Error::InvalidState("placement verification failed".into()));
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

/// Run placement normalization with an initially minimized window.
pub fn run_place_minimized_test(timeout_ms: u64, with_logs: bool) -> Result<()> {
    run_place_with_state(timeout_ms, with_logs, true, false, "M".to_string())
}

/// Run placement normalization with an initially zoomed (maximized) window.
pub fn run_place_zoomed_test(timeout_ms: u64, with_logs: bool) -> Result<()> {
    run_place_with_state(timeout_ms, with_logs, false, true, "Z".to_string())
}
