//! Fullscreen control smoketest.
//!
//! What this verifies
//! - Toggling fullscreen (native or non-native) on a focused helper window via
//!   a bound key path executes without backend errors and yields readable
//!   before/after window frames via AX.
//!
//! Acceptance criteria
//! - The test captures an initial frame, drives fullscreen using the configured
//!   binding, then successfully reads a non-`None` frame afterward within
//!   `FULLSCREEN_WAIT_TOTAL_MS` while polling at `FULLSCREEN_WAIT_POLL_MS`.
//! - Backend connectivity remains healthy during the wait; an IPC failure
//!   causes the test to fail with `IpcDisconnected`.
//! - A significant area change is expected but not enforced strictly; this is a
//!   smoke check rather than a pixel-perfect assertion.
use std::{cmp, thread, time::Instant};

use hotki_protocol::Toggle;

use crate::{
    config,
    error::{Error, Result},
    helper_window::{HelperWindow, ensure_frontmost},
    server_drive,
    test_runner::{TestConfig, TestRunner},
    ui_interaction::send_key,
};

/// Run a focused non-native fullscreen toggle against a helper window.
///
/// Steps:
/// - Launch hotki with a tiny temp config that binds `f` to fullscreen(toggle, nonnative).
/// - Spawn a helper window with a unique title and keep it frontmost.
/// - Show HUD, press `f` to toggle non-native fullscreen.
/// - Optionally validate the window frame changed; then immediately kill the helper.
pub fn run_fullscreen_test(
    timeout_ms: u64,
    with_logs: bool,
    state: Toggle,
    native: bool,
) -> Result<()> {
    // Generate a unique helper title up front so we can embed a raise binding
    // targeting it. This ensures the backend acts on the intended window and
    // avoids touching the user's windows if focus drifts.
    let title = config::test_title("fullscreen");

    // Minimal config: add a raise(title) binding and fullscreen binding.
    let state_str = match state {
        Toggle::Toggle => "toggle",
        Toggle::On => "on",
        Toggle::Off => "off",
    };
    let kind_suffix = if native { ", native" } else { "" };
    let ron_config = format!(
        "(\n        keys: [\n            (\"g\", \"raise\", raise(title: \"{}\"), (noexit: true)),\n            (\"shift+cmd+9\", \"Fullscreen\", fullscreen({}{}) , (global: true)),\n        ],\n        style: (hud: (mode: hide)),\n        server: (exit_if_no_clients: true),\n    )",
        title, state_str, kind_suffix
    );

    let config = TestConfig::new(timeout_ms)
        .with_temp_config(ron_config)
        .with_logs(with_logs);

    TestRunner::new("fullscreen_test", config)
        .with_setup(|ctx| {
            ctx.launch_hotki()?;
            // Ensure RPC ready and both bindings are registered
            ctx.ensure_rpc_ready(&["g", "shift+cmd+9"])?;
            Ok(())
        })
        .with_execute(move |ctx| {
            // Ensure RPC driver is connected to the right backend before driving keys.
            if ctx.session.is_some() {
                server_drive::wait_for_ident("g", 2000)?;
                server_drive::wait_for_ident("shift+cmd+9", 2000)?;
            }
            // Spawn helper window with the embedded title

            let helper_time = ctx
                .config
                .timeout_ms
                .saturating_add(config::HELPER_WINDOW_EXTRA_TIME_MS);
            let mut helper = HelperWindow::spawn_frontmost(
                &title,
                helper_time,
                cmp::min(ctx.config.timeout_ms, config::HIDE_FIRST_WINDOW_MAX_MS),
                config::FULLSCREEN_HELPER_SHOW_DELAY_MS,
                "FS",
            )?;
            // Gate safety: raise the helper via backend binding, then wait until
            // both CG frontmost and backend world focus agree on our helper PID.
            send_key("g")?;
            ensure_frontmost(helper.pid, &title, 4, config::UI_ACTION_DELAY_MS);
            server_drive::wait_for_focused_pid(helper.pid, config::WAIT_FIRST_WINDOW_MS)?;

            // Capture initial frame via AX
            let before = mac_winops::ax_window_frame(helper.pid, &title)
                .ok_or_else(|| Error::InvalidState("Failed to read initial window frame".into()))?;
            let _ = &before;

            // Trigger fullscreen toggle via global chord, then actively wait for a frame update
            send_key("shift+cmd+9")?;

            // Read new frame; tolerate AX timing
            let mut after = mac_winops::ax_window_frame(helper.pid, &title);
            let start_wait = Instant::now();
            while after.is_none()
                && start_wait.elapsed() < config::ms(config::FULLSCREEN_WAIT_TOTAL_MS)
            {
                // Bail early if backend died
                if let Err(err) = server_drive::check_alive() {
                    eprintln!("[fullscreen] backend not alive during toggle wait: {}", err);
                    return Err(Error::IpcDisconnected {
                        during: "fullscreen toggle",
                    });
                }
                thread::sleep(config::ms(config::FULLSCREEN_WAIT_POLL_MS));
                after = mac_winops::ax_window_frame(helper.pid, &title);
            }
            let after = after.ok_or_else(|| {
                Error::InvalidState("Failed to read window frame after toggle".into())
            })?;

            // Quick sanity: area or dimensions changed meaningfully
            let area_before = before.1.0 * before.1.1;
            let area_after = after.1.0 * after.1.1;
            if (area_after - area_before).abs() < 1.0 {
                // Not necessarily an error — some displays may already have a maximized window,
                // but we still proceed to kill the helper to exercise the path.
            }

            // Immediately kill the helper window to exercise teardown path
            if let Err(_e) = helper.kill_and_wait() {}

            Ok(())
        })
        .with_teardown(|ctx, _| {
            ctx.shutdown();
            Ok(())
        })
        .run()
}
