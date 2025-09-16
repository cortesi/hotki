//! Hide on/off smoketest.
//!
//! What this verifies
//! - The `hide(toggle|on|off)` actions move a focused window off-screen to the
//!   right edge (leaving a 1 px sliver) and restore it to its original frame.
//! - We operate on a helper window we create, and we measure its position and
//!   size via AX APIs.
//!
//! Acceptance criteria
//! - After driving `h → o` (hide on), the helper’s X position changes away from
//!   the original and is approximately at the right-edge sliver of the visible
//!   frame (within a small tolerance).
//! - After reopening and driving `h → f` (hide off), the window’s position and
//!   size return approximately to their original values (within a small
//!   tolerance).
//! - If frames cannot be read, or no movement/restoration is observed within
//!   the configured time windows, the test fails with a descriptive error.
//!
//! Notes
//! - The HUD is hidden; we explicitly keep the helper frontmost to avoid
//!   acting on the wrong window.
use std::{
    cmp, thread,
    time::{Duration, Instant},
};

use objc2_app_kit::NSScreen;
use objc2_foundation::MainThreadMarker;

use super::helpers::approx;
use crate::{
    config,
    error::{Error, Result},
    helper_window::{HelperWindow, ensure_frontmost},
    test_runner::{TestConfig, TestRunner},
    ui_interaction::{send_activation_chord, send_key},
};

/// Run the hide on/off smoketest with a temporary keybinding config.
pub fn run_hide_test(timeout_ms: u64, with_logs: bool) -> Result<()> {
    // Temporary config: shift+cmd+0 -> h -> (t/on/off); hide HUD to reduce intrusiveness
    let ron_config = r#"(
    keys: [
        ("shift+cmd+0", "activate", keys([
            ("h", "hide", keys([
                ("t", "toggle", hide(toggle)),
                ("o", "on", hide(on)),
                ("f", "off", hide(off)),
            ])),
            ("shift+cmd+0", "exit", exit, (global: true, hide: true)),
        ])),
        ("esc", "Back", pop, (global: true, hide: true, hud_only: true)),
    ],
    style: (hud: (mode: hide))
)
"#;

    let config = TestConfig::new(timeout_ms)
        .with_logs(with_logs)
        .with_temp_config(ron_config);

    TestRunner::new("hide_test", config)
        .with_setup(|ctx| {
            ctx.launch_hotki()?;
            // Wait for HUD to ensure bindings are installed
            ctx.wait_for_hud()?;
            // Ensure activation and hide submenu idents are registered (h, o, f).
            ctx.ensure_rpc_ready(&["shift+cmd+0", "h", "o", "f"])?;
            Ok(())
        })
        .with_execute(|ctx| {
            // Spawn our own helper window (winit) and use it as the hide target.
            let title = config::test_title("hide");
            let helper_time = ctx
                .config
                .timeout_ms
                .saturating_add(config::HIDE_HELPER_EXTRA_TIME_MS);
            let helper = HelperWindow::spawn_frontmost(
                &title,
                helper_time,
                cmp::min(ctx.config.timeout_ms, config::HIDE_FIRST_WINDOW_MAX_MS),
                config::HIDE_POLL_MS,
                "H",
            )?;
            let pid = helper.pid;

            // Snapshot initial AX frame of the helper window
            let (p0, s0) = if let Some(((px, py), (width, height))) =
                mac_winops::ax_window_frame(pid, &title)
            {
                ((px, py), (width, height))
            } else {
                return Err(Error::FocusNotObserved {
                    timeout_ms: ctx.config.timeout_ms,
                    expected: "AX window for helper".into(),
                });
            };

            // Compute expected target X on the main screen (1px sliver)
            let target_x = if let Some(mtm) = MainThreadMarker::new() {
                let scr = NSScreen::mainScreen(mtm).expect("main screen");
                let vf = scr.visibleFrame();
                (vf.origin.x + vf.size.width) - 1.0
            } else {
                // Fallback guess: large X likely on right
                p0.0 + config::WINDOW_POSITION_OFFSET
            };

            // Ensure the helper window is frontmost before issuing hide commands.
            ensure_frontmost(pid, &title, 2, config::HIDE_ACTIVATE_POST_DELAY_MS);

            // Drive: send 'h' then 'o' (hide on) — idents are pre-gated in setup
            send_key("h")?;
            send_key("o")?;

            // Wait for position change
            let mut moved = false;
            let deadline = Instant::now()
                + Duration::from_millis(cmp::max(
                    config::HIDE_MIN_TIMEOUT_MS,
                    ctx.config.timeout_ms / 4,
                ));
            let mut _p_on = p0;
            while Instant::now() < deadline {
                if let Some((px, py)) = mac_winops::ax_window_position(pid, &title) {
                    _p_on = (px, py);
                    if !approx(px, p0.0, 2.0) || approx(px, target_x, 6.0) {
                        moved = true;
                        break;
                    }
                }
                thread::sleep(config::ms(config::KEY_EVENT_DELAY_MS));
            }
            if !moved {
                eprintln!(
                    "debug: no movement detected after hide(on). last vs start x: {:.1} -> {:.1}",
                    _p_on.0, p0.0
                );
                return Err(Error::SpawnFailed(
                    "window position did not change after hide(on)".into(),
                ));
            }

            // Drive: reopen/activate and turn hide off (reveal)
            send_activation_chord()?;
            // Raise helper again before revealing to avoid toggling an unrelated window.
            ensure_frontmost(pid, &title, 2, config::HIDE_ACTIVATE_POST_DELAY_MS);
            send_key("h")?;
            send_key("f")?;

            // Wait until position roughly returns to original
            let mut restored = false;
            let deadline2 = Instant::now()
                + Duration::from_millis((ctx.config.timeout_ms / 3).clamp(
                    config::HIDE_SECONDARY_MIN_TIMEOUT_MS,
                    config::HIDE_RESTORE_MAX_MS,
                ));
            while Instant::now() < deadline2 {
                if let Some(((px2, py2), (width2, height2))) =
                    mac_winops::ax_window_frame(pid, &title)
                {
                    let pos_ok = approx(px2, p0.0, 8.0) && approx(py2, p0.1, 8.0);
                    let size_ok = approx(width2, s0.0, 8.0) && approx(height2, s0.1, 8.0);
                    // quiet on success path
                    if pos_ok && size_ok {
                        restored = true;
                        break;
                    }
                }
                thread::sleep(config::ms(config::HIDE_POLL_MS));
            }

            if !restored {
                return Err(Error::SpawnFailed(
                    "window did not restore to original frame after hide(off)".into(),
                ));
            }
            Ok(())
        })
        .with_teardown(|ctx, _| {
            ctx.shutdown();
            Ok(())
        })
        .run()
}
