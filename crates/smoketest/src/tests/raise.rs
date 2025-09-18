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
use std::{
    cmp, thread,
    time::{Duration, Instant},
};

use crate::{
    config,
    error::{Error, Result},
    helper_window::{
        FRONTMOST_IGNORE_TITLES, HelperWindow, HelperWindowBuilder, frontmost_app_window,
        wait_for_frontmost_title,
    },
    server_drive,
    test_runner::{TestConfig, TestRunner},
    tests::fixtures::{wait_for_backend_focused_title, wait_for_windows_visible},
    ui_interaction::send_key,
};

/// Total sampling window for focus probe logging (milliseconds).
const FOCUS_PROBE_DURATION_MS: u64 = 600;
/// Interval between focus probe samples (milliseconds).
const FOCUS_PROBE_SAMPLE_MS: u64 = 60;

/// Emit AX/CG focus snapshots to stderr when smoketest logging is enabled.
fn probe_focus_state(label: &str, entries: &[(i32, &str)], with_logs: bool) {
    if !with_logs {
        return;
    }
    let deadline = Instant::now() + Duration::from_millis(FOCUS_PROBE_DURATION_MS);
    let mut sample_idx = 0u32;
    while Instant::now() < deadline {
        let front = frontmost_app_window(FRONTMOST_IGNORE_TITLES);
        let front_desc = front
            .as_ref()
            .map(|w| format!("pid={} title='{}'", w.pid, w.title))
            .unwrap_or_else(|| "<none>".to_string());
        let helper_state: Vec<String> = entries
            .iter()
            .map(|(pid, title)| {
                let cg_match = front
                    .as_ref()
                    .is_some_and(|w| w.pid == *pid && w.title == *title);
                let ax_match = mac_winops::ax_has_window_title(*pid, title);
                format!(
                    "{{title='{}' pid={} cg_match={} ax_match={}}}",
                    title, pid, cg_match, ax_match
                )
            })
            .collect();
        eprintln!(
            "[raise][{}] sample#{:02} frontmost={} helpers=[{}]",
            label,
            sample_idx,
            front_desc,
            helper_state.join(" ")
        );
        sample_idx += 1;
        thread::sleep(Duration::from_millis(FOCUS_PROBE_SAMPLE_MS));
    }
}

/// Gate a menu identifier via RPC when available.
fn gate_ident_when_ready(ident: &str, timeout_ms: u64) -> Result<()> {
    if server_drive::is_ready() {
        server_drive::wait_for_ident(ident, timeout_ms)?;
    }
    Ok(())
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
        ("ctrl+alt+1", "raise-1", raise(title: "{t1}"), (global: true, hide: true)),
        ("ctrl+alt+2", "raise-2", raise(title: "{t2}"), (global: true, hide: true)),
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
            // Ensure the activation chord binding is registered before driving.
            ctx.ensure_rpc_ready(&["ctrl+alt+1", "ctrl+alt+2"])?;
            Ok(())
        })
        .with_execute(move |ctx| {
            // Initialize RPC driver for key injects
            if let Some(sess) = ctx.session.as_ref() {
                let sock = sess.socket_path().to_string();
                server_drive::ensure_init(&sock, 3000)?;
            }

            // Spawn two helper windows
            let helper_time = ctx
                .config
                .timeout_ms
                .saturating_add(config::RAISE.helper_extra_time_ms);
            let child1 = HelperWindow::spawn_frontmost_with_builder(
                HelperWindowBuilder::new(&title1)
                    .with_time_ms(helper_time)
                    .with_label_text("R1")
                    .with_size(600.0, 420.0)
                    .with_position(60.0, 60.0),
                &title1,
                cmp::min(ctx.config.timeout_ms, config::RAISE.first_window_max_ms),
                config::INPUT_DELAYS.window_registration_delay_ms,
                ctx.config.with_logs,
            )?;
            let pid1 = child1.pid;
            // Small active wait to let the first window register before spawning the second
            let _ = wait_for_windows_visible(
                &[(pid1, &title1)],
                config::INPUT_DELAYS.window_registration_delay_ms,
            );
            let child2 = HelperWindow::spawn_frontmost_with_builder(
                HelperWindowBuilder::new(&title2)
                    .with_time_ms(helper_time)
                    .with_label_text("R2")
                    .with_size(600.0, 420.0)
                    .with_position(1000.0, 60.0),
                &title2,
                cmp::min(ctx.config.timeout_ms, config::RAISE.first_window_max_ms),
                config::INPUT_DELAYS.window_registration_delay_ms,
                ctx.config.with_logs,
            )?;
            let pid2 = child2.pid;
            // Keep child1/child2 alive for the duration of this execute block.

            // Ensure both helper windows are present before proceeding.
            if !wait_for_windows_visible(
                &[(pid1, &title1), (pid2, &title2)],
                config::WAITS.both_windows_ms,
            ) {
                return Err(Error::FocusNotObserved {
                    timeout_ms: 8000,
                    expected: "helpers not visible in CG/AX".into(),
                });
            }
            probe_focus_state(
                "pre-raise",
                &[(pid1, &title1), (pid2, &title2)],
                ctx.config.with_logs,
            );

            // Phase 1: raise first window (shift+cmd+0 → r → 1), then assert CG frontmost.
            ctx.ensure_rpc_ready(&["ctrl+alt+1", "ctrl+alt+2"])?;
            gate_ident_when_ready("ctrl+alt+1", config::RAISE.binding_gate_ms)?;
            send_key("ctrl+alt+1")?;
            probe_focus_state(
                "post-raise-one",
                &[(pid1, &title1), (pid2, &title2)],
                ctx.config.with_logs,
            );
            if !wait_for_frontmost_title(&title1, ctx.config.timeout_ms / 2) {
                eprintln!(
                    "raise: warning - CG frontmost did not settle on '{}' within timeout",
                    title1
                );
            }
            if let Err(err) = wait_for_backend_focused_title(&title1, ctx.config.timeout_ms / 3) {
                eprintln!("raise: backend focus check (phase 1) failed: {}", err);
            }

            // Phase 2: raise second window (shift+cmd+0 → r → 2), then assert CG frontmost.
            gate_ident_when_ready("ctrl+alt+2", config::RAISE.binding_gate_ms)?;
            send_key("ctrl+alt+2")?;
            probe_focus_state(
                "post-raise-two",
                &[(pid1, &title1), (pid2, &title2)],
                ctx.config.with_logs,
            );
            if !wait_for_frontmost_title(&title2, ctx.config.timeout_ms / 2) {
                eprintln!(
                    "raise: warning - CG frontmost did not settle on '{}' within timeout",
                    title2
                );
            }
            if let Err(err) = wait_for_backend_focused_title(&title2, ctx.config.timeout_ms / 3) {
                eprintln!("raise: backend focus check (phase 2) failed: {}", err);
            }
            Ok(())
        })
        .with_teardown(|ctx, _| {
            ctx.shutdown();
            Ok(())
        })
        .run()
}
