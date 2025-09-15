//! Raise-by-title smoketest.
//!
//! Goal
//! - Verify that the `raise(title: ...)` action brings the requested window to
//!   the front based on its title.
//!
//! How
//! - Spawn two helper windows with unique titles.
//! - Open the temporary raise menu (`shift+cmd+0 → r`) and raise the first
//!   window with `1`, then reopen and raise the second with `2`.
//! - Acceptance is based on the actual frontmost CG window title. The backend
//!   focus title (via RPC) is optionally checked for additional signal but does
//!   not drive pass/fail.
//!
//! Acceptance
//! - Both helpers are visible (CG or AX) before attempting to raise.
//! - After the first raise, the CG frontmost title equals `title1` within the
//!   step timeout; after the second raise, it equals `title2`.
use std::cmp;

use super::helpers::{wait_for_backend_focused_title, wait_for_windows_visible};
use crate::{
    config,
    error::{Error, Result},
    helper_window::{HelperWindow, HelperWindowBuilder, wait_for_frontmost_title},
    server_drive,
    test_runner::{TestConfig, TestRunner},
    ui_interaction::send_key,
};

/// Gate a menu identifier via RPC when available.
fn gate_ident_when_ready(ident: &str, timeout_ms: u64) {
    if server_drive::is_ready() {
        let _ = server_drive::wait_for_ident(ident, timeout_ms);
    }
}

/// Run the raise-by-title smoketest using a temporary activation menu.
pub fn run_raise_test(timeout_ms: u64, with_logs: bool) -> Result<()> {
    // Two unique titles
    let title1 = config::test_title("raise-1");
    let title2 = config::test_title("raise-2");

    // Build a temporary config enabling raise by title under: shift+cmd+0 -> r -> 1/2
    let ron_config = format!(
        r#"(
    keys: [
        ("shift+cmd+0", "activate", keys([
            ("r", "raise", keys([
                ("1", "one", raise(title: "{t1}")),
                ("2", "two", raise(title: "{t2}")),
            ])),
            ("shift+cmd+0", "exit", exit, (global: true, hide: true)),
        ])),
        ("esc", "Back", pop, (global: true, hide: true, hud_only: true)),
    ]
    , style: (hud: (mode: hide))
)
"#,
        t1 = title1,
        t2 = title2
    );
    let config = TestConfig::new(timeout_ms)
        .with_logs(with_logs)
        .with_temp_config(ron_config);

    TestRunner::new("raise_test", config)
        .with_setup(|ctx| {
            ctx.launch_hotki()?;
            // Ensure RPC ready for the root 'r' binding
            let _ = ctx.ensure_rpc_ready(&["r"]);
            Ok(())
        })
        .with_execute(move |ctx| {
            // Initialize RPC driver for key injects
            if let Some(sess) = ctx.session.as_ref() {
                let sock = sess.socket_path().to_string();
                let _ = server_drive::ensure_init(&sock, 3000);
            }

            // Spawn two helper windows
            let helper_time = ctx
                .config
                .timeout_ms
                .saturating_add(config::RAISE_HELPER_EXTRA_MS);
            let child1 = HelperWindow::spawn_frontmost_with_builder(
                HelperWindowBuilder::new(&title1)
                    .with_time_ms(helper_time)
                    .with_label_text("R1")
                    .with_size(600.0, 420.0)
                    .with_position(60.0, 60.0),
                &title1,
                cmp::min(ctx.config.timeout_ms, config::RAISE_FIRST_WINDOW_MAX_MS),
                config::WINDOW_REGISTRATION_DELAY_MS,
            )?;
            let pid1 = child1.pid;
            // Small active wait to let the first window register before spawning the second
            let _ =
                wait_for_windows_visible(&[(pid1, &title1)], config::WINDOW_REGISTRATION_DELAY_MS);
            let child2 = HelperWindow::spawn_frontmost_with_builder(
                HelperWindowBuilder::new(&title2)
                    .with_time_ms(helper_time)
                    .with_label_text("R2")
                    .with_size(600.0, 420.0)
                    .with_position(1000.0, 60.0),
                &title2,
                cmp::min(ctx.config.timeout_ms, config::RAISE_FIRST_WINDOW_MAX_MS),
                config::WINDOW_REGISTRATION_DELAY_MS,
            )?;
            let pid2 = child2.pid;
            // Keep child1/child2 alive for the duration of this execute block.

            // Ensure both helper windows are present before proceeding.
            if !wait_for_windows_visible(
                &[(pid1, &title1), (pid2, &title2)],
                config::WAIT_BOTH_WINDOWS_MS,
            ) {
                return Err(Error::FocusNotObserved {
                    timeout_ms: 8000,
                    expected: "helpers not visible in CG/AX".into(),
                });
            }

            // Phase 1: raise first window (shift+cmd+0 → r → 1), then assert CG frontmost.
            let _ = ctx.ensure_rpc_ready(&["shift+cmd+0", "r"]);
            send_key("shift+cmd+0");
            gate_ident_when_ready("r", config::RAISE_BINDING_GATE_MS);
            send_key("r");
            if !wait_for_windows_visible(&[(pid1, &title1)], config::RAISE_FIRST_WINDOW_MAX_MS) {
                return Err(Error::FocusNotObserved {
                    timeout_ms: 6000,
                    expected: format!("first window not visible before menu: '{}'", title1),
                });
            }
            gate_ident_when_ready("1", config::RAISE_BINDING_GATE_MS);
            send_key("1");
            if !wait_for_frontmost_title(&title1, ctx.config.timeout_ms / 2) {
                return Err(Error::FocusNotObserved {
                    timeout_ms: ctx.config.timeout_ms,
                    expected: title1,
                });
            }
            let _ = wait_for_backend_focused_title(&title1, ctx.config.timeout_ms / 3);

            // Phase 2: raise second window (shift+cmd+0 → r → 2), then assert CG frontmost.
            send_key("shift+cmd+0");
            gate_ident_when_ready("r", config::RAISE_BINDING_GATE_MS);
            send_key("r");
            if !wait_for_windows_visible(&[(pid2, &title2)], config::RAISE_FIRST_WINDOW_MAX_MS) {
                return Err(Error::FocusNotObserved {
                    timeout_ms: 6000,
                    expected: format!("second window not visible before menu: '{}'", title2),
                });
            }
            gate_ident_when_ready("2", config::RAISE_BINDING_GATE_MS);
            send_key("2");
            if !wait_for_frontmost_title(&title2, ctx.config.timeout_ms / 2) {
                return Err(Error::FocusNotObserved {
                    timeout_ms: ctx.config.timeout_ms,
                    expected: title2,
                });
            }
            let _ = wait_for_backend_focused_title(&title2, ctx.config.timeout_ms / 3);
            Ok(())
        })
        .with_teardown(|ctx, _| {
            ctx.shutdown();
            Ok(())
        })
        .run()
}
