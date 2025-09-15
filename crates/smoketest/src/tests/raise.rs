//! Raise-by-title smoketest.
//!
//! What this verifies
//! - The `raise(title: ...)`` action brings the targeted window to the front.
//! - Two helper windows with unique titles are spawned; we drive `r → 1` to
//!   raise the first, then `r → 2` to raise the second.
//! - Focus detection is cross-checked via two mechanisms: the system’s
//!   frontmost window title (CG) and HUDUpdate events from the backend.
//!
//! Acceptance criteria
//! - Both helper windows become visible (CG or AX) before attempting to raise.
//! - After `r → 1`, the frontmost window matches `title1` within the per-step
//!   timeout; after `r → 2`, it matches `title2` within the per-step timeout.
//! - If the backend IPC disconnects while waiting for events, the test fails
//!   with `IpcDisconnected`.
//! - If the expected focus is not observed in time, the test fails with
//!   `FocusNotObserved { expected, timeout_ms }`.
//!
//! Notes
//! - The test gates on server identifiers to avoid racing binding
//!   registration, and retries key paths once if necessary.
use std::{
    cmp,
    time::{Duration, Instant},
};

use hotki_server::Client;
use tokio::time::timeout;

use super::helpers::{HelperWindow, wait_for_frontmost_title, wait_for_windows_visible};
use crate::{
    config,
    error::{Error, Result},
    process::HelperWindowBuilder,
    runtime, server_drive,
    test_runner::{TestConfig, TestRunner},
    ui_interaction::send_key,
};

/// Wait for a HUD update with `expected` title within `timeout_ms`.
/// Prefer a lightweight RPC snapshot via the shared driver; fall back to
/// event stream only if the driver is not initialized.
async fn wait_for_title(sock: &str, expected: &str, timeout_ms: u64) -> Result<bool> {
    // Fast path: use shared RPC snapshot polling
    if server_drive::is_ready() {
        return Ok(server_drive::wait_for_focused_title(expected, timeout_ms));
    }
    // Fallback: transient client and HudUpdate stream

    let mut client = match Client::new_with_socket(sock)
        .with_connect_only()
        .connect()
        .await
    {
        Ok(c) => c,
        Err(_) => {
            return Err(Error::IpcDisconnected {
                during: "connecting for title events",
            });
        }
    };
    let conn = match client.connection() {
        Ok(c) => c,
        Err(_) => {
            return Err(Error::IpcDisconnected {
                during: "waiting for title events",
            });
        }
    };

    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    while Instant::now() < deadline {
        let left = deadline.saturating_duration_since(Instant::now());
        let chunk = cmp::min(left, config::ms(config::RETRY_DELAY_MS));
        match timeout(chunk, conn.recv_event()).await {
            Ok(Ok(hotki_protocol::MsgToUI::HudUpdate { cursor })) => {
                if let Some(app) = cursor.app_ref()
                    && app.title == expected
                {
                    return Ok(true);
                }
            }
            Ok(Ok(_)) => {}
            Ok(Err(_)) => {
                return Err(Error::IpcDisconnected {
                    during: "waiting for title events",
                });
            }
            Err(_) => {}
        }
    }
    Ok(false)
}

// Prefer checking the actual frontmost CG window title; this validates raise
// independent of our HUD event flow and avoids races.
// Use helpers::wait_for_frontmost_title

// Wait until all given (pid,title) pairs are present in the on-screen CG list.
// Use helpers::wait_for_windows_visible

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

            // Ensure both helper windows are actually present before proceeding
            if !wait_for_windows_visible(
                &[(pid1, &title1), (pid2, &title2)],
                config::WAIT_BOTH_WINDOWS_MS,
            ) {
                return Err(Error::FocusNotObserved {
                    timeout_ms: 8000,
                    expected: "helpers not visible in CG/AX".into(),
                });
            }

            // Ensure activation and 'r' are registered; open HUD, then enter raise menu.
            let _ = ctx.ensure_rpc_ready(&["shift+cmd+0", "r"]);
            send_key("shift+cmd+0");
            send_key("r");
            // Ensure the first helper is visible (CG or AX) before issuing '1'
            if !wait_for_windows_visible(
                &[(pid1, &title1)],
                config::WAIT_FIRST_WINDOW_MS.min(config::RAISE_FIRST_WINDOW_MAX_MS),
            ) {
                return Err(Error::FocusNotObserved {
                    timeout_ms: 6000,
                    expected: format!("first window not visible before menu: '{}'", title1),
                });
            }
            // Wait for '1' binding to appear under 'raise' if driving via RPC
            let _ = ctx.ensure_rpc_ready(&["1"]);
            send_key("1");

            // Wait for focus to title1 (prefer frontmost CG check; fall back to HUD)
            let sock = ctx
                .session
                .as_ref()
                .ok_or_else(|| Error::InvalidState("No session".into()))?
                .socket_path()
                .to_string();
            let ok1_front = wait_for_frontmost_title(&title1, ctx.config.timeout_ms / 2);
            let ok1 = if ok1_front
                || runtime::block_on(wait_for_title(&sock, &title1, ctx.config.timeout_ms / 2))??
            {
                true
            } else {
                // Actively wait for the '1' binding under raise, then retry
                let _ = ctx.ensure_rpc_ready(&["1"]);
                send_key("1");
                wait_for_frontmost_title(&title1, ctx.config.timeout_ms / 2)
                    || runtime::block_on(wait_for_title(
                        &sock,
                        &title1,
                        ctx.config.timeout_ms / 2,
                    ))??
            };
            if !ok1 {
                // Final robust attempt: reopen HUD and try raise again
                send_key("shift+cmd+0");
                let _ = ctx.ensure_rpc_ready(&["r"]);
                send_key("r");
                let _ =
                    wait_for_windows_visible(&[(pid1, &title1)], config::RAISE_WINDOW_RECHECK_MS);
                send_key("1");
                let ok1_retry = wait_for_frontmost_title(&title1, ctx.config.timeout_ms / 2)
                    || runtime::block_on(wait_for_title(
                        &sock,
                        &title1,
                        ctx.config.timeout_ms / 2,
                    ))??;
                if !ok1_retry {
                    return Err(Error::FocusNotObserved {
                        timeout_ms: ctx.config.timeout_ms,
                        expected: title1,
                    });
                }
            }

            // Reopen HUD and raise second window
            send_key("shift+cmd+0");
            let _ = ctx.ensure_rpc_ready(&["r"]);
            // Ensure the second helper is visible (CG or AX) before issuing '2'
            if !wait_for_windows_visible(&[(pid2, &title2)], config::RAISE_FIRST_WINDOW_MAX_MS) {
                return Err(Error::FocusNotObserved {
                    timeout_ms: 6000,
                    expected: format!("second window not visible before menu: '{}'", title2),
                });
            }
            send_key("r");
            let _ = ctx.ensure_rpc_ready(&["2"]);
            send_key("2");
            let ok2_front = wait_for_frontmost_title(&title2, ctx.config.timeout_ms / 2);
            let mut ok2 = if ok2_front
                || runtime::block_on(wait_for_title(&sock, &title2, ctx.config.timeout_ms / 2))??
            {
                true
            } else {
                // Actively wait for the '2' binding again, then retry immediately
                if server_drive::is_ready() {
                    let _ = server_drive::wait_for_ident("2", config::RAISE_BINDING_GATE_MS);
                }
                send_key("2");
                wait_for_frontmost_title(&title2, ctx.config.timeout_ms / 2)
                    || runtime::block_on(wait_for_title(
                        &sock,
                        &title2,
                        ctx.config.timeout_ms / 2,
                    ))??
            };
            if !ok2 {
                // Final robust attempt for the second window as well
                send_key("shift+cmd+0");
                if server_drive::is_ready() {
                    let _ = server_drive::wait_for_ident("r", config::RAISE_BINDING_GATE_MS);
                }
                send_key("r");
                let _ =
                    wait_for_windows_visible(&[(pid2, &title2)], config::RAISE_WINDOW_RECHECK_MS);
                send_key("2");
                ok2 = wait_for_frontmost_title(&title2, ctx.config.timeout_ms / 2)
                    || runtime::block_on(wait_for_title(
                        &sock,
                        &title2,
                        ctx.config.timeout_ms / 2,
                    ))??;
            }

            if !ok2 {
                return Err(Error::FocusNotObserved {
                    timeout_ms: ctx.config.timeout_ms,
                    expected: title2,
                });
            }
            Ok(())
        })
        .with_teardown(|ctx, _| {
            ctx.shutdown();
            Ok(())
        })
        .run()
}
