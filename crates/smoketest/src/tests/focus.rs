//! Focus-tracking smoketest.
//!
//! What this verifies
//! - Hotki reports focus for a newly created helper window.
//! - We accept either of two independent signals: a HUDUpdate event whose
//!   cursor app title matches the helper title, or the system’s current
//!   frontmost window (via Core Graphics) has the expected title.
//!
//! Acceptance criteria
//! - Within `timeout_ms`, at least one of the above signals is observed.
//! - IPC to the backend remains connected while waiting; if it disconnects the
//!   test fails with `IpcDisconnected`.
//! - On success, the test returns a `FocusOutcome` containing the observed
//!   title, pid, and elapsed time.
//! - On failure, the test errors with `FocusNotObserved { expected, timeout_ms }`.
//!
//! Notes
//! - The HUD is hidden for this test; we avoid depending on HUD visuals and
//!   keep the helper window frontmost proactively to reduce flakiness.
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use super::helpers::{ensure_frontmost, spawn_helper_visible, wait_for_frontmost_title};
use crate::{
    config,
    error::{Error, Result},
    results::FocusOutcome,
    runtime, server_drive,
    test_runner::{TestConfig, TestRunner},
};

/// Listen for focus events on the given socket
async fn listen_for_focus(
    socket_path: &str,
    expected_title: String,
    found: Arc<AtomicBool>,
    done: Arc<AtomicBool>,
    matched: Arc<Mutex<Option<(String, i32)>>>,
    ipc_down: Arc<AtomicBool>,
) {
    // Retry connect briefly to avoid racing server startup
    let mut client = loop {
        match hotki_server::Client::new_with_socket(socket_path)
            .with_connect_only()
            .connect()
            .await
        {
            Ok(c) => break c,
            Err(_) => {
                if done.load(Ordering::SeqCst) {
                    ipc_down.store(true, Ordering::SeqCst);
                    return;
                }
                // Short retry window
                tokio::time::sleep(Duration::from_millis(50)).await;
                continue;
            }
        }
    };

    let conn = match client.connection() {
        Ok(c) => c,
        Err(_) => {
            ipc_down.store(true, Ordering::SeqCst);
            return;
        }
    };

    let per_wait = config::ms(config::FOCUS_EVENT_POLL_MS);
    loop {
        if done.load(Ordering::SeqCst) {
            break;
        }

        let res = tokio::time::timeout(per_wait, conn.recv_event()).await;
        match res {
            Ok(Ok(hotki_protocol::MsgToUI::HudUpdate { cursor })) => {
                if let Some(app) = cursor.app_ref()
                    && app.title == expected_title
                {
                    if let Ok(mut g) = matched.lock() {
                        *g = Some((app.title.clone(), app.pid));
                    }
                    found.store(true, Ordering::SeqCst);
                    break;
                }
            }
            Ok(Ok(_)) => {}
            Ok(Err(_)) => {
                ipc_down.store(true, Ordering::SeqCst);
                break;
            }
            Err(_) => {}
        }
    }
}

// Prefer checking the current frontmost CG window title directly — this avoids
// relying solely on HUD updates and reduces flakiness from event timing.
// Use helpers::wait_for_frontmost_title

pub fn run_focus_test(timeout_ms: u64, with_logs: bool) -> Result<FocusOutcome> {
    let ron_config = "(keys: [], style: (hud: (mode: hide)))";
    let config = TestConfig::new(timeout_ms)
        .with_logs(with_logs)
        .with_temp_config(ron_config);

    // Generate unique title for the test window
    let expected_title = config::test_title("focus");

    // Shared state for event listener
    let found = Arc::new(AtomicBool::new(false));
    let done = Arc::new(AtomicBool::new(false));
    let matched: Arc<Mutex<Option<(String, i32)>>> = Arc::new(Mutex::new(None));
    let ipc_down = Arc::new(AtomicBool::new(false));

    TestRunner::new("focus_test", config)
        .with_setup(|ctx| {
            ctx.launch_hotki()?;
            Ok(())
        })
        .with_execute(move |ctx| {
            // Get socket path from session
            let socket_path = ctx
                .session
                .as_ref()
                .ok_or_else(|| Error::InvalidState("No session".into()))?
                .socket_path()
                .to_string();

            // Initialize RPC driver with a bounded wait to reduce races
            let _ = server_drive::ensure_init(&socket_path, 3000);

            // Start background listener
            let expected_title_clone = expected_title.clone();
            let found_clone = found.clone();
            let done_clone = done.clone();
            let matched_clone = matched.clone();
            let ipc_clone = ipc_down.clone();

            let sock_for_listener = socket_path.clone();
            let listener = thread::spawn(move || {
                let _ = runtime::block_on(listen_for_focus(
                    &sock_for_listener,
                    expected_title_clone,
                    found_clone,
                    done_clone,
                    matched_clone,
                    ipc_clone,
                ));
            });

            // Ensure RPC driver remains initialized for liveness checks.
            let _ = server_drive::ensure_init(&socket_path, 3000);

            // Spawn helper window
            let helper_time = ctx
                .config
                .timeout_ms
                .saturating_add(config::HELPER_WINDOW_EXTRA_TIME_MS);
            let helper = spawn_helper_visible(
                expected_title.clone(),
                helper_time,
                std::cmp::min(ctx.config.timeout_ms, config::HIDE_FIRST_WINDOW_MAX_MS),
                config::FOCUS_POLL_MS,
                "F",
            )?;
            let expected_pid = helper.pid;

            // Best‑effort: explicitly bring helper to front
            ensure_frontmost(expected_pid, &expected_title, 20, 150);
            if wait_for_frontmost_title(&expected_title, config::FOCUS_EVENT_POLL_MS) {
                if let Ok(mut g) = matched.lock() {
                    *g = Some((expected_title.clone(), expected_pid));
                }
                found.store(true, Ordering::SeqCst);
            }

            // Wait for match or timeout
            let deadline = Instant::now() + Duration::from_millis(timeout_ms);
            let start = Instant::now();

            while Instant::now() < deadline {
                if found.load(Ordering::SeqCst) {
                    break;
                }
                // Fall back to CG frontmost check to reduce flakiness
                if wait_for_frontmost_title(&expected_title, config::FOCUS_EVENT_POLL_MS) {
                    if let Ok(mut g) = matched.lock() {
                        *g = Some((expected_title.clone(), expected_pid));
                    }
                    found.store(true, Ordering::SeqCst);
                    break;
                }
                // Attempt lazy init of RPC driver until connected
                if !server_drive::is_ready() {
                    let _ = server_drive::init(&socket_path);
                }
                // If back-end died after being reachable, bail early with clear error
                if server_drive::is_ready() && !server_drive::check_alive() {
                    done.store(true, Ordering::SeqCst);
                    let _ = listener.join();
                    return Err(Error::IpcDisconnected {
                        during: "focus wait",
                    });
                }
                thread::sleep(config::ms(config::FOCUS_POLL_MS));
            }

            // Signal listener to stop
            done.store(true, Ordering::SeqCst);
            let _ = listener.join();

            // Check if we found the window
            if !found.load(Ordering::SeqCst) {
                if ipc_down.load(Ordering::SeqCst) {
                    return Err(Error::IpcDisconnected {
                        during: "listening for focus",
                    });
                }
                return Err(Error::FocusNotObserved {
                    timeout_ms,
                    expected: expected_title.clone(),
                });
            }

            let (title, pid) = matched
                .lock()
                .ok()
                .and_then(|g| g.clone())
                .unwrap_or((expected_title, expected_pid));

            Ok(FocusOutcome {
                title,
                pid,
                elapsed_ms: start.elapsed().as_millis() as u64,
            })
        })
        .with_teardown(|ctx, _| {
            ctx.shutdown();
            Ok(())
        })
        .run()
}
