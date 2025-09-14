//! Placement normalization smoketests: exercise minimized/zoomed pre-states.

use std::time::{SystemTime, UNIX_EPOCH};

use super::{geom, helpers::spawn_helper_with_options};
use crate::{
    config,
    error::{Error, Result},
    test_runner::{TestConfig, TestRunner},
    ui_interaction::send_key,
};

// Geometry and polling helpers are provided by `tests::geom`.

fn run_place_with_state(
    timeout_ms: u64,
    with_logs: bool,
    start_minimized: bool,
    start_zoomed: bool,
    label: String,
) -> Result<()> {
    let cols = crate::config::PLACE_COLS;
    let rows = crate::config::PLACE_ROWS;
    let now_pre = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let helper_title = format!(
        "hotki smoketest: place-state {}-{}",
        std::process::id(),
        now_pre
    );
    // Minimal config: raise + place (0,0) on key '1'
    let ron_config = format!(
        "(\n    keys: [\n        (\"g\", \"raise\", raise(title: \"{}\"), (noexit: true)),\n        (\"1\", \"(0,0)\", place(grid({}, {}), at(0, 0))),\n    ],\n    style: (hud: (mode: hide)),\n    server: (exit_if_no_clients: true),\n)\n",
        helper_title, cols, rows
    );
    let config = TestConfig::new(timeout_ms)
        .with_logs(with_logs)
        .with_temp_config(ron_config);

    TestRunner::new("place_state", config)
        .with_setup(|ctx| {
            ctx.launch_hotki()?;
            // Ensure RPC ready and the required bindings are registered before driving.
            let _ = ctx.ensure_rpc_ready(&["g", "1"]);
            Ok(())
        })
        .with_execute(move |ctx| {
            let title = helper_title.clone();
            let helper_time = ctx
                .config
                .timeout_ms
                .saturating_add(config::HELPER_WINDOW_EXTRA_TIME_MS);
            let mut helper = spawn_helper_with_options(
                title.clone(),
                helper_time,
                std::cmp::min(ctx.config.timeout_ms, config::HIDE_FIRST_WINDOW_MAX_MS),
                config::PLACE_POLL_MS,
                &label,
                start_minimized,
                start_zoomed,
            )?;

            // Ensure frontmost by driving the raise binding a few times and waiting
            for _ in 0..3 {
                send_key("g");
                if super::helpers::wait_for_frontmost_title(&title, config::WAIT_FIRST_WINDOW_MS) {
                    break;
                }
            }

            // If the helper started minimized, AX frame can lag after de-miniaturize.
            // Actively wait until an AX frame is available before issuing placement.
            let ready_deadline = std::time::Instant::now()
                + std::time::Duration::from_millis(std::cmp::min(
                    ctx.config.timeout_ms,
                    config::WAIT_FIRST_WINDOW_MS,
                ));
            while std::time::Instant::now() < ready_deadline
                && mac_winops::ax_window_frame(helper.pid, &title).is_none()
            {
                std::thread::sleep(config::ms(config::PLACE_POLL_MS));
            }

            let (vf_x, vf_y, vf_w, vf_h) = if let Some((px, py)) =
                mac_winops::ax_window_position(helper.pid, &title)
                && let Some(vf) = geom::visible_frame_containing_point(px, py)
            {
                vf
            } else if let Some(vf) = geom::resolve_vf_for_window(
                helper.pid,
                &title,
                crate::config::DEFAULT_TIMEOUT_MS,
                crate::config::PLACE_POLL_MS,
            ) {
                vf
            } else {
                return Err(Error::InvalidState(
                    "Failed to resolve screen visibleFrame".into(),
                ));
            };

            // Expected cell rect for (0,0)
            let (ex, ey, ew, eh) = geom::cell_rect((vf_x, vf_y, vf_w, vf_h), cols, rows, 0, 0);
            send_key("1");
            let ok = geom::wait_for_expected_frame(
                helper.pid,
                &title,
                (ex, ey, ew, eh),
                config::PLACE_EPS,
                config::PLACE_STEP_TIMEOUT_MS,
                config::PLACE_POLL_MS,
            );
            if !ok {
                return Err(Error::InvalidState("placement verification failed".into()));
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

pub fn run_place_minimized_test(timeout_ms: u64, with_logs: bool) -> Result<()> {
    run_place_with_state(timeout_ms, with_logs, true, false, "M".to_string())
}

pub fn run_place_zoomed_test(timeout_ms: u64, with_logs: bool) -> Result<()> {
    run_place_with_state(timeout_ms, with_logs, false, true, "Z".to_string())
}
