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

    // Give the system a moment to settle new windows into the CG list
    std::thread::sleep(Duration::from_millis(1000));
    // Navigate to raise menu: already at root after shift+cmd+0; press r then 1
    send_key("r");
    std::thread::sleep(Duration::from_millis(150));
    send_key("1");

    // Wait for focus to title1 (retry once for robustness)
    let ok1 = {
        let rt = tokio::runtime::Runtime::new().map_err(SmkError::Io)?;
        let first = rt.block_on(wait_for_title(sess.socket_path(), &title1, timeout_ms / 2));
        if first {
            true
        } else {
            std::thread::sleep(Duration::from_millis(300));
            send_key("1");
            rt.block_on(wait_for_title(sess.socket_path(), &title1, timeout_ms / 2))
        }
    };
    if !ok1 {
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

    // Reopen HUD and raise second window
    std::thread::sleep(Duration::from_millis(250));
    send_key("shift+cmd+0");
    std::thread::sleep(Duration::from_millis(200));
    send_key("r");
    std::thread::sleep(Duration::from_millis(120));
    send_key("2");
    let ok2 = {
        let rt = tokio::runtime::Runtime::new().map_err(SmkError::Io)?;
        let first = rt.block_on(wait_for_title(sess.socket_path(), &title2, timeout_ms / 2));
        if first {
            true
        } else {
            std::thread::sleep(Duration::from_millis(300));
            send_key("2");
            rt.block_on(wait_for_title(sess.socket_path(), &title2, timeout_ms / 2))
        }
    };

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
