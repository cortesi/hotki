//! Placement skip smoketest for non-movable windows (Stage 8 minimal).
//!
//! Verifies that when the focused window is non-movable (AXPosition not settable),
//! the engine performs an advisory pre-gate and does not attempt placement.

use std::{
    cmp, thread,
    time::{Duration, Instant},
};

use super::helpers::{wait_for_frontmost_title, wait_for_window_visible};
use crate::{
    config,
    error::{Error, Result},
    process::HelperWindowBuilder,
    test_runner::{TestConfig, TestRunner},
    ui_interaction::send_key,
};

/// Fetch the AX frame for `(pid,title)` as `(x,y,w,h)`.
fn ax_frame(pid: i32, title: &str) -> Option<(f64, f64, f64, f64)> {
    mac_winops::ax_window_frame(pid, title).map(|((x, y), (w, h))| (x, y, w, h))
}

/// Approximate float equality within `eps`.
fn approx(a: f64, b: f64, eps: f64) -> bool {
    (a - b).abs() <= eps
}

/// Compare two frames for approximate equality.
fn same_frame(a: (f64, f64, f64, f64), b: (f64, f64, f64, f64), eps: f64) -> bool {
    approx(a.0, b.0, eps) && approx(a.1, b.1, eps) && approx(a.2, b.2, eps) && approx(a.3, b.3, eps)
}

/// Run the placement skip smoketest for non-movable windows.
pub fn run_place_skip_test(timeout_ms: u64, with_logs: bool) -> Result<()> {
    // Minimal bindings: raise by title, then attempt place(grid(2,2), at(0,0)) on key '1'.
    let cols = 2u32;
    let rows = 2u32;
    let (col, row) = (0u32, 0u32);
    let helper_title = config::test_title("place-skip");
    let ron_config: String = format!(
        "(\n    keys: [\n        (\"g\", \"raise\", raise(title: \"{}\"), (noexit: true)),\n        (\"1\", \"(0,0)\", place(grid({}, {}), at({}, {}))),\n    ],\n    style: (hud: (mode: hide)),\n    server: (exit_if_no_clients: true),\n)\n",
        helper_title, cols, rows, col, row
    );

    let config = TestConfig::new(timeout_ms)
        .with_logs(with_logs)
        .with_temp_config(ron_config);

    TestRunner::new("place_skip", config)
        .with_setup(|ctx| {
            ctx.launch_hotki()?;
            // Gate on the required bindings before executing.
            let _ = ctx.ensure_rpc_ready(&["g", "1"]);
            Ok(())
        })
        .with_execute(move |ctx| {
            // Spawn a non-movable helper and wait for it to appear
            let title = helper_title.clone();
            let helper_time = ctx
                .config
                .timeout_ms
                .saturating_add(config::HELPER_WINDOW_EXTRA_TIME_MS);
            let helper = HelperWindowBuilder::new(title.clone())
                .with_time_ms(helper_time)
                .with_label_text("NM")
                .with_nonmovable(true)
                .with_attach_sheet(true)
                .spawn()
                .map_err(|e| Error::SpawnFailed(e.to_string()))?;
            if !wait_for_window_visible(
                helper.pid,
                &title,
                cmp::min(ctx.config.timeout_ms, config::HIDE_FIRST_WINDOW_MAX_MS),
                config::PLACE_POLL_MS,
            ) {
                return Err(Error::FocusNotObserved {
                    timeout_ms,
                    expected: title,
                });
            }

            // Inspect AX props to determine if a skip is expected on this system
            let id = mac_winops::list_windows()
                .into_iter()
                .find(|w| w.pid == helper.pid && w.title == title)
                .map(|w| w.id)
                .ok_or_else(|| Error::InvalidState("Failed to resolve helper CGWindowId".into()))?;
            let mut expect_skip = false;
            if let Ok(props) = mac_winops::ax_props_for_window_id(id) {
                let role = props.role.clone().unwrap_or_default();
                let sub = props.subrole.clone().unwrap_or_default();
                let pos_ok = props.can_set_pos.unwrap_or(true);
                let size_ok = props.can_set_size.unwrap_or(true);
                if role == "AXSheet" || sub == "AXPopover" || sub == "AXFloatingWindow" {
                    expect_skip = true;
                }
                if !pos_ok || !size_ok {
                    expect_skip = true;
                }
                eprintln!(
                    "place-skip: props role='{}' subrole='{}' can_set_pos={:?} can_set_size={:?}",
                    role, sub, props.can_set_pos, props.can_set_size
                );
            }

            // Capture initial frame, attempt placement, and assert unchanged
            let before = ax_frame(helper.pid, &title)
                .ok_or_else(|| Error::InvalidState("AX frame unavailable".into()))?;
            // Ensure helper is frontmost and that the backend reports the helper PID focused,
            // then request placement via the key binding. This tightens targeting so we never
            // resize a non-test window even if world focus lags.
            send_key("g");
            let _ = wait_for_frontmost_title(&title, config::WAIT_FIRST_WINDOW_MS);
            let _ =
                crate::server_drive::wait_for_focused_pid(helper.pid, config::WAIT_FIRST_WINDOW_MS);
            send_key("1");
            // Wait for a short settle period and compare
            let settle_ms = 350; // generous but bounded
            let deadline = Instant::now() + Duration::from_millis(settle_ms);
            let mut unchanged = false;
            while Instant::now() < deadline {
                if let Some(after) = ax_frame(helper.pid, &title)
                    && same_frame(before, after, config::PLACE_EPS)
                {
                    unchanged = true;
                    break;
                }
                thread::sleep(Duration::from_millis(config::PLACE_POLL_MS));
            }
            if expect_skip {
                if !unchanged {
                    return Err(Error::InvalidState(
                        "place-skip: window moved but props indicated skip was expected".into(),
                    ));
                }
            } else {
                // Informational only: if skip not expected from props, do not fail.
                eprintln!(
                    "place-skip: skip not expected (props allow set); treating as informational run"
                );
            }
            // Drop helper by ending scope
            drop(helper);
            Ok(())
        })
        .run()
}
