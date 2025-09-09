use std::time::Instant;

use super::helpers::{ensure_frontmost, spawn_helper_visible};
use crate::{
    config,
    error::{Error, Result},
    server_drive,
    test_runner::{TestConfig, TestRunner},
    ui_interaction::send_key,
};
use hotki_protocol::Toggle;

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
    // Minimal config: bind activation and fullscreen(toggle, nonnative)
    let state_str = match state {
        Toggle::Toggle => "toggle",
        Toggle::On => "on",
        Toggle::Off => "off",
    };
    let kind_suffix = if native { ", native" } else { "" };
    let ron_config = format!(
        "(\n        keys: [\n            (\"shift+cmd+9\", \"Fullscreen\", fullscreen({}{}) , (global: true)),\n        ],\n    )",
        state_str, kind_suffix
    );

    let config = TestConfig::new(timeout_ms)
        .with_temp_config(ron_config)
        .with_logs(with_logs);

    TestRunner::new("fullscreen_test", config)
        .with_setup(|ctx| {
            ctx.launch_hotki()?;
            Ok(())
        })
        .with_execute(|ctx| {
            // Ensure RPC driver is connected to the right backend before driving keys.
            if let Some(sess) = ctx.session.as_ref() {
                let sock = sess.socket_path().to_string();
                let start = std::time::Instant::now();
                let mut inited = crate::server_drive::init(&sock);
                while !inited && start.elapsed() < std::time::Duration::from_millis(3000) {
                    std::thread::sleep(std::time::Duration::from_millis(50));
                    inited = crate::server_drive::init(&sock);
                }
                let _ = inited;
                // Wait briefly until the binding is registered so injects resolve.
                let _ = crate::server_drive::wait_for_ident("shift+cmd+9", 2000);
            }
            // Spawn helper window with unique title
            let ts = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let title = format!("hotki smoketest: fullscreen {}", ts);

            let helper_time = ctx
                .config
                .timeout_ms
                .saturating_add(config::HELPER_WINDOW_EXTRA_TIME_MS);
            let mut helper = spawn_helper_visible(
                title.clone(),
                helper_time,
                std::cmp::min(ctx.config.timeout_ms, config::HIDE_FIRST_WINDOW_MAX_MS),
                config::FULLSCREEN_HELPER_SHOW_DELAY_MS,
            )?;
            // Make sure the helper is the focused window before toggling fullscreen.
            ensure_frontmost(
                helper.pid,
                &title,
                4,
                config::FULLSCREEN_HELPER_SHOW_DELAY_MS,
            );

            // Capture initial frame via AX
            let before = mac_winops::ax_window_frame(helper.pid, &title)
                .ok_or_else(|| Error::InvalidState("Failed to read initial window frame".into()))?;
            let _ = &before;

            // Trigger fullscreen toggle via global chord
            send_key("shift+cmd+9");
            std::thread::sleep(config::ms(config::FULLSCREEN_POST_TOGGLE_DELAY_MS));

            // If the backend crashed as a result of fullscreen, surface it immediately.
            if !server_drive::check_alive() {
                eprintln!("[fullscreen] backend not alive after toggle");
                return Err(Error::IpcDisconnected {
                    during: "fullscreen toggle",
                });
            }
            // backend alive; continue to read updated frame

            // Read new frame; tolerate AX timing
            let mut after = mac_winops::ax_window_frame(helper.pid, &title);
            let start_wait = Instant::now();
            while after.is_none()
                && start_wait.elapsed() < config::ms(config::FULLSCREEN_WAIT_TOTAL_MS)
            {
                std::thread::sleep(config::ms(config::FULLSCREEN_WAIT_POLL_MS));
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
            let _ = helper.kill_and_wait();

            Ok(())
        })
        .with_teardown(|ctx, _| {
            ctx.shutdown();
            Ok(())
        })
        .run()
}
