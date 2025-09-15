//! UI demo smoketests.
//!
//! What this verifies
//! - `run_ui_demo`: Launch the UI with a test config, wait for the HUD to
//!   become ready, then drive a small theme-cycle sequence (show tester, next
//!   theme a few times, back, exit).
//! - `run_minui_demo`: Same as above, but with the mini HUD mode.
//!
//! Acceptance criteria
//! - The HUD is observed (readiness gate satisfied) and a `Summary` is
//!   returned with `hud_seen = true` and `time_to_hud_ms` set.
//! - The driving sequence completes without backend errors, and the session is
//!   cleanly torn down.
use std::iter::repeat_n;

use crate::{
    error::Result,
    results::Summary,
    test_runner::{TestConfig, TestRunner},
    ui_interaction::send_key_sequence,
};

/// Run the standard UI demo test.
pub fn run_ui_demo(timeout_ms: u64) -> Result<Summary> {
    // Keep HUD visible and anchor it to the bottom-right (se) for this demo.
    let ron_config = r#"(
        keys: [
            ("shift+cmd+0", "activate", keys([
                ("t", "Theme tester", keys([
                    ("h", "Theme Prev", theme_prev, (noexit: true)),
                    ("l", "Theme Next", theme_next, (noexit: true)),
                ])),
            ])),
            ("shift+cmd+0", "exit", exit, (global: true, hide: true)),
            ("esc", "Back", pop, (global: true, hide: true, hud_only: true)),
        ],
        style: (hud: (mode: hud, pos: se)),
    )"#;

    let config = TestConfig::new(timeout_ms)
        .with_temp_config(ron_config)
        .with_logs(true);

    TestRunner::new("ui_demo", config)
        .with_setup(|ctx| {
            ctx.launch_hotki()?;
            // Ensure the activation chord and 't' menu binding are registered before driving.
            let _ = ctx.ensure_rpc_ready(&["shift+cmd+0", "t"]);
            Ok(())
        })
        .with_execute(|ctx| {
            let time_to_hud = ctx.wait_for_hud()?;
            // Send key sequence to test UI
            let mut seq: Vec<&str> = Vec::new();
            seq.push("t");
            seq.extend(repeat_n("l", 5));
            seq.push("esc");
            seq.push("shift+cmd+0");
            send_key_sequence(&seq);

            let mut sum = Summary::new();
            sum.hud_seen = true;
            sum.time_to_hud_ms = Some(time_to_hud);
            Ok(sum)
        })
        .with_teardown(|ctx, _| {
            ctx.shutdown();
            Ok(())
        })
        .run()
}

/// Run the mini UI demo test.
pub fn run_minui_demo(timeout_ms: u64) -> Result<Summary> {
    let ron_config = r#"(
        keys: [
            ("shift+cmd+0", "activate", keys([
                ("t", "Theme tester", keys([
                    ("h", "Theme Prev", theme_prev, (noexit: true)),
                    ("l", "Theme Next", theme_next, (noexit: true)),
                ])),
            ])),
            ("shift+cmd+0", "exit", exit, (global: true, hide: true)),
            ("esc", "Back", pop, (global: true, hide: true, hud_only: true)),
        ],
        style: (hud: (mode: mini, pos: se)),
    )"#;

    let config = TestConfig::new(timeout_ms)
        .with_temp_config(ron_config)
        .with_logs(false);

    TestRunner::new("minui_demo", config)
        .with_setup(|ctx| {
            ctx.launch_hotki()?;
            // Ensure the activation chord and 't' menu binding are registered before driving.
            let _ = ctx.ensure_rpc_ready(&["shift+cmd+0", "t"]);
            Ok(())
        })
        .with_execute(|ctx| {
            let time_to_hud = ctx.wait_for_hud()?;
            // Send key sequence to test mini UI
            let mut seq: Vec<&str> = Vec::new();
            seq.push("t");
            seq.extend(repeat_n("l", 5));
            seq.push("esc");
            seq.push("shift+cmd+0");
            send_key_sequence(&seq);

            let mut sum = Summary::new();
            sum.hud_seen = true;
            sum.time_to_hud_ms = Some(time_to_hud);
            Ok(sum)
        })
        .with_teardown(|ctx, _| {
            ctx.shutdown();
            Ok(())
        })
        .run()
}
