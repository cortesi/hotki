use std::{
    env, fs,
    process::{Command, Stdio},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use crate::{SmkError, session::HotkiSession, util::resolve_hotki_bin};

fn send_key(seq: &str) {
    if let Some(ch) = mac_keycode::Chord::parse(seq) {
        let rk = relaykey::RelayKey::new_unlabeled();
        let pid = 0; // global post
        rk.key_down(pid, ch.clone(), false);
        std::thread::sleep(Duration::from_millis(60));
        rk.key_up(pid, ch);
    }
}

async fn wait_for_title(sock: &str, expected: &str, timeout_ms: u64) -> bool {
    use hotki_server::Client;

    let mut client = match Client::new_with_socket(sock)
        .with_connect_only()
        .connect()
        .await
    {
        Ok(c) => c,
        Err(_) => return false,
    };
    let conn = match client.connection() {
        Ok(c) => c,
        Err(_) => return false,
    };

    let deadline = std::time::Instant::now() + Duration::from_millis(timeout_ms);
    while std::time::Instant::now() < deadline {
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        let chunk = std::cmp::min(left, Duration::from_millis(300));
        match tokio::time::timeout(chunk, conn.recv_event()).await {
            Ok(Ok(hotki_protocol::MsgToUI::HudUpdate { cursor })) => {
                if let Some(app) = cursor.app_ref()
                    && app.title == expected
                {
                    return true;
                }
            }
            Ok(Ok(_)) => {}
            Ok(Err(_)) => return false,
            Err(_) => {}
        }
    }
    false
}

// Prefer checking the actual frontmost CG window title; this validates raise
// independent of our HUD event flow and avoids races.
fn wait_for_frontmost_title(expected: &str, timeout_ms: u64) -> bool {
    let deadline = std::time::Instant::now() + Duration::from_millis(timeout_ms);
    while std::time::Instant::now() < deadline {
        if let Some(win) = mac_winops::frontmost_window()
            && win.title == expected
        {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    false
}

// Wait until all given (pid,title) pairs are present in the on-screen CG list.
fn wait_for_windows(expected: &[(i32, &str)], timeout_ms: u64) -> bool {
    let deadline = std::time::Instant::now() + Duration::from_millis(timeout_ms);
    while std::time::Instant::now() < deadline {
        let wins = mac_winops::list_windows();
        let all_found = expected.iter().all(|(pid, title)| {
            let cg_present = wins.iter().any(|w| w.pid == *pid || w.title == *title);
            let ax_present = mac_winops::ax_has_window_title(*pid, title);
            cg_present || ax_present
        });
        if all_found {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    // Debug: print current windows for diagnosis
    let wins = mac_winops::list_windows();
    eprintln!("debug: visible windows:");
    for w in wins {
        eprintln!("  pid={} app='{}' title='{}'", w.pid, w.app, w.title);
    }
    false
}

pub(crate) fn run_raise_test(timeout_ms: u64, with_logs: bool) -> Result<(), SmkError> {
    let Some(hotki_bin) = resolve_hotki_bin() else {
        return Err(SmkError::HotkiBinNotFound);
    };

    // Two unique titles
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let title1 = format!("hotki smoketest: raise-1 {}-{}", std::process::id(), now);
    let title2 = format!("hotki smoketest: raise-2 {}-{}", std::process::id(), now);

    // Spawn two helper windows
    let helper_time = timeout_ms.saturating_add(8000);
    let exe = env::current_exe().map_err(SmkError::Io)?;
    let mut child1 = Command::new(&exe)
        .arg("focus-winhelper")
        .arg("--title")
        .arg(&title1)
        .arg("--time")
        .arg(helper_time.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| SmkError::SpawnFailed(e.to_string()))?;
    // Small stagger to avoid simultaneous window registration races in WindowServer
    std::thread::sleep(Duration::from_millis(200));
    let mut child2 = Command::new(&exe)
        .arg("focus-winhelper")
        .arg("--title")
        .arg(&title2)
        .arg("--time")
        .arg(helper_time.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| SmkError::SpawnFailed(e.to_string()))?;

    let pid1 = child1.id() as i32;
    let pid2 = child2.id() as i32;

    // Ensure both helper windows are actually present before proceeding
    if !wait_for_windows(&[(pid1, &title1), (pid2, &title2)], 8000) {
        let _ = child1.kill();
        let _ = child2.kill();
        let _ = child1.wait();
        let _ = child2.wait();
        return Err(SmkError::FocusNotObserved {
            timeout_ms: 8000,
            expected: "helpers not visible in CG/AX".into(),
        });
    }

    // Do not pre-gate here; we'll wait right before driving the raise keys.

    // Build a temporary config enabling raise by title under: shift+cmd+0 -> r -> 1/2
    let cfg = format!(
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
)
"#,
        t1 = title1,
        t2 = title2
    );
    let tmp_path = env::temp_dir().join(format!("hotki-smoketest-raise-{}.ron", now));
    fs::write(&tmp_path, cfg).map_err(SmkError::Io)?;

    // Launch session and wait for HUD
    let mut sess = HotkiSession::launch_with_config(&hotki_bin, &tmp_path, with_logs)?;
    let (hud_ok, _ms) = sess.wait_for_hud(timeout_ms);
    if !hud_ok {
        sess.shutdown();
        sess.kill_and_wait();
        let _ = child1.kill();
        let _ = child2.kill();
        let _ = child1.wait();
        let _ = child2.wait();
        return Err(SmkError::HudNotVisible { timeout_ms });
    }

    // Best‑effort: allow WindowServer to register helpers before driving raise
    // Navigate to raise menu: already at root after shift+cmd+0; press r then 1
    send_key("r");
    // Ensure the first helper is visible (CG or AX) before issuing '1'
    if !wait_for_windows(&[(pid1, &title1)], 6000) {
        sess.shutdown();
        sess.kill_and_wait();
        let _ = child1.kill();
        let _ = child2.kill();
        let _ = child1.wait();
        let _ = child2.wait();
        return Err(SmkError::FocusNotObserved {
            timeout_ms: 6000,
            expected: format!("first window not visible before menu: '{}'", title1),
        });
    }
    send_key("1");

    // Wait for focus to title1 (prefer frontmost CG check; fall back to HUD)
    let ok1_front = wait_for_frontmost_title(&title1, timeout_ms / 2);
    let ok1 = if ok1_front {
        true
    } else {
        let rt = tokio::runtime::Runtime::new().map_err(SmkError::Io)?;
        let ok_hud = rt.block_on(wait_for_title(sess.socket_path(), &title1, timeout_ms / 2));
        if ok_hud {
            true
        } else {
            std::thread::sleep(Duration::from_millis(300));
            send_key("1");
            // One more attempt using CG frontmost first, then HUD
            wait_for_frontmost_title(&title1, timeout_ms / 2)
                || rt.block_on(wait_for_title(sess.socket_path(), &title1, timeout_ms / 2))
        }
    };
    if !ok1 {
        // Final robust attempt: reopen HUD and try raise again
        send_key("shift+cmd+0");
        std::thread::sleep(Duration::from_millis(150));
        send_key("r");
        let _ = wait_for_windows(&[(pid1, &title1)], 1500);
        send_key("1");
        let ok1_retry = wait_for_frontmost_title(&title1, timeout_ms / 2) || {
            let rt = tokio::runtime::Runtime::new().map_err(SmkError::Io)?;
            rt.block_on(wait_for_title(sess.socket_path(), &title1, timeout_ms / 2))
        };
        if !ok1_retry {
            sess.shutdown();
            sess.kill_and_wait();
            let _ = child1.kill();
            let _ = child2.kill();
            let _ = child1.wait();
            let _ = child2.wait();
            return Err(SmkError::FocusNotObserved {
                timeout_ms,
                expected: title1,
            });
        }
    }

    // Reopen HUD and raise second window
    std::thread::sleep(Duration::from_millis(250));
    send_key("shift+cmd+0");
    // Ensure the second helper is visible (CG or AX) before issuing '2'
    if !wait_for_windows(&[(pid2, &title2)], 6000) {
        sess.shutdown();
        sess.kill_and_wait();
        let _ = child1.kill();
        let _ = child2.kill();
        let _ = child1.wait();
        let _ = child2.wait();
        return Err(SmkError::FocusNotObserved {
            timeout_ms: 6000,
            expected: format!("second window not visible before menu: '{}'", title2),
        });
    }
    send_key("r");
    std::thread::sleep(Duration::from_millis(120));
    send_key("2");
    let ok2_front = wait_for_frontmost_title(&title2, timeout_ms / 2);
    let mut ok2 = if ok2_front {
        true
    } else {
        let rt = tokio::runtime::Runtime::new().map_err(SmkError::Io)?;
        let ok_hud = rt.block_on(wait_for_title(sess.socket_path(), &title2, timeout_ms / 2));
        if ok_hud {
            true
        } else {
            std::thread::sleep(Duration::from_millis(300));
            send_key("2");
            wait_for_frontmost_title(&title2, timeout_ms / 2)
                || rt.block_on(wait_for_title(sess.socket_path(), &title2, timeout_ms / 2))
        }
    };
    if !ok2 {
        // Final robust attempt for the second window as well
        send_key("shift+cmd+0");
        std::thread::sleep(Duration::from_millis(150));
        send_key("r");
        let _ = wait_for_windows(&[(pid2, &title2)], 1500);
        send_key("2");
        ok2 = wait_for_frontmost_title(&title2, timeout_ms / 2) || {
            let rt = tokio::runtime::Runtime::new().map_err(SmkError::Io)?;
            rt.block_on(wait_for_title(sess.socket_path(), &title2, timeout_ms / 2))
        };
    }

    // Cleanup
    sess.shutdown();
    sess.kill_and_wait();
    let _ = child1.kill();
    let _ = child2.kill();
    let _ = child1.wait();
    let _ = child2.wait();
    let _ = fs::remove_file(&tmp_path);

    if !ok2 {
        return Err(SmkError::FocusNotObserved {
            timeout_ms,
            expected: title2,
        });
    }
    Ok(())
}
