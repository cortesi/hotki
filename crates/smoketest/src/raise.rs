use std::{
    cmp, env, fs,
    path::PathBuf,
    process::{self, Child, Command, Stdio},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use crate::{
    error::{Error, Result},
    session::HotkiSession,
    util::resolve_hotki_bin,
};

// Tunable timings for UI-driven interactions
const POLL_MS: u64 = 50;
const KEY_POST_DELAY_MS: u64 = 60;
const RETRY_DELAY_MS: u64 = 300;
const OPEN_MENU_STAGGER_MS: u64 = 150;
const WINDOW_REG_STAGGER_MS: u64 = 200;
const AFTER_MENU_STABILIZE_MS: u64 = 250;
const BETWEEN_MENU_KEYS_MS: u64 = 120;
const WAIT_WINDOWS_BOTH_MS: u64 = 15000;
const WAIT_WINDOWS_FIRST_MS: u64 = 6000;
const WAIT_WINDOWS_RECHECK_MS: u64 = 1500;

struct Cleanup {
    child1: Option<Child>,
    child2: Option<Child>,
    tmp_path: Option<PathBuf>,
}

impl Cleanup {
    fn new() -> Self {
        Self {
            child1: None,
            child2: None,
            tmp_path: None,
        }
    }
}

impl Drop for Cleanup {
    fn drop(&mut self) {
        if let Some(mut c) = self.child1.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
        if let Some(mut c) = self.child2.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
        if let Some(p) = self.tmp_path.take() {
            let _ = fs::remove_file(p);
        }
    }
}

struct SessionGuard {
    sess: *mut HotkiSession,
}

impl SessionGuard {
    fn new(sess: &mut HotkiSession) -> Self {
        Self { sess }
    }
}

impl Drop for SessionGuard {
    fn drop(&mut self) {
        unsafe {
            if !self.sess.is_null() {
                (*self.sess).shutdown();
                (*self.sess).kill_and_wait();
            }
        }
    }
}

fn send_key(seq: &str) {
    if let Some(ch) = mac_keycode::Chord::parse(seq) {
        let rk = relaykey::RelayKey::new_unlabeled();
        let pid = 0; // global post
        rk.key_down(pid, ch.clone(), false);
        thread::sleep(Duration::from_millis(KEY_POST_DELAY_MS));
        rk.key_up(pid, ch);
    }
}

fn spawn_helper(exe: &PathBuf, title: &str, time_ms: u64) -> Result<process::Child> {
    let child = Command::new(exe)
        .arg("focus-winhelper")
        .arg("--title")
        .arg(title)
        .arg("--time")
        .arg(time_ms.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| Error::SpawnFailed(e.to_string()))?;
    Ok(child)
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

    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    while Instant::now() < deadline {
        let left = deadline.saturating_duration_since(Instant::now());
        let chunk = cmp::min(left, Duration::from_millis(RETRY_DELAY_MS));
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
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    while Instant::now() < deadline {
        if let Some(win) = mac_winops::frontmost_window()
            && win.title == expected
        {
            return true;
        }
        thread::sleep(Duration::from_millis(POLL_MS));
    }
    false
}

// Wait until all given (pid,title) pairs are present in the on-screen CG list.
fn wait_for_windows(expected: &[(i32, &str)], timeout_ms: u64) -> bool {
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    while Instant::now() < deadline {
        let wins = mac_winops::list_windows();
        let all_found = expected.iter().all(|(pid, title)| {
            let cg_present = wins.iter().any(|w| w.pid == *pid && w.title == *title);
            let ax_present = mac_winops::ax_has_window_title(*pid, title);
            cg_present || ax_present
        });
        if all_found {
            return true;
        }
        thread::sleep(Duration::from_millis(POLL_MS));
    }
    // Debug: print current windows for diagnosis
    let wins = mac_winops::list_windows();
    eprintln!("debug: visible windows:");
    for w in wins {
        eprintln!("  pid={} app='{}' title='{}'", w.pid, w.app, w.title);
    }
    false
}

pub(crate) fn run_raise_test(timeout_ms: u64, with_logs: bool) -> Result<()> {
    let Some(hotki_bin) = resolve_hotki_bin() else {
        return Err(Error::HotkiBinNotFound);
    };

    // Two unique titles
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let title1 = format!("hotki smoketest: raise-1 {}-{}", process::id(), now);
    let title2 = format!("hotki smoketest: raise-2 {}-{}", process::id(), now);

    // Spawn two helper windows
    let helper_time = timeout_ms.saturating_add(8000);
    let exe = env::current_exe()?;
    let mut cleanup = Cleanup::new();
    let child1 = spawn_helper(&exe, &title1, helper_time)?;
    // Small stagger to avoid simultaneous window registration races in WindowServer
    thread::sleep(Duration::from_millis(WINDOW_REG_STAGGER_MS));
    let child2 = spawn_helper(&exe, &title2, helper_time)?;

    let pid1 = child1.id() as i32;
    let pid2 = child2.id() as i32;
    cleanup.child1 = Some(child1);
    cleanup.child2 = Some(child2);

    // Ensure both helper windows are actually present before proceeding
    if !wait_for_windows(&[(pid1, &title1), (pid2, &title2)], WAIT_WINDOWS_BOTH_MS) {
        return Err(Error::FocusNotObserved {
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
    fs::write(&tmp_path, cfg)?;
    cleanup.tmp_path = Some(tmp_path.clone());

    // Launch session and wait for HUD
    let mut sess = HotkiSession::launch_with_config(&hotki_bin, &tmp_path, with_logs)?;
    let _sess_guard = SessionGuard::new(&mut sess);
    let (hud_ok, _ms) = sess.wait_for_hud(timeout_ms);
    if !hud_ok {
        return Err(Error::HudNotVisible { timeout_ms });
    }

    // Reuse a single Tokio runtime for HUD event waits
    let rt = tokio::runtime::Runtime::new().map_err(|e| Error::Io(e.into()))?;

    // Best‑effort: allow WindowServer to register helpers before driving raise
    // Navigate to raise menu: already at root after shift+cmd+0; press r then 1
    send_key("r");
    // Ensure the first helper is visible (CG or AX) before issuing '1'
    if !wait_for_windows(&[(pid1, &title1)], WAIT_WINDOWS_FIRST_MS) {
        return Err(Error::FocusNotObserved {
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
        let ok_hud = rt.block_on(wait_for_title(sess.socket_path(), &title1, timeout_ms / 2));
        if ok_hud {
            true
        } else {
            thread::sleep(Duration::from_millis(RETRY_DELAY_MS));
            send_key("1");
            // One more attempt using CG frontmost first, then HUD
            wait_for_frontmost_title(&title1, timeout_ms / 2)
                || rt.block_on(wait_for_title(sess.socket_path(), &title1, timeout_ms / 2))
        }
    };
    if !ok1 {
        // Final robust attempt: reopen HUD and try raise again
        send_key("shift+cmd+0");
        thread::sleep(Duration::from_millis(OPEN_MENU_STAGGER_MS));
        send_key("r");
        let _ = wait_for_windows(&[(pid1, &title1)], WAIT_WINDOWS_RECHECK_MS);
        send_key("1");
        let ok1_retry = wait_for_frontmost_title(&title1, timeout_ms / 2)
            || rt.block_on(wait_for_title(sess.socket_path(), &title1, timeout_ms / 2));
        if !ok1_retry {
            return Err(Error::FocusNotObserved {
                timeout_ms,
                expected: title1,
            });
        }
    }

    // Reopen HUD and raise second window
    thread::sleep(Duration::from_millis(AFTER_MENU_STABILIZE_MS));
    send_key("shift+cmd+0");
    // Ensure the second helper is visible (CG or AX) before issuing '2'
    if !wait_for_windows(&[(pid2, &title2)], 6000) {
        return Err(Error::FocusNotObserved {
            timeout_ms: 6000,
            expected: format!("second window not visible before menu: '{}'", title2),
        });
    }
    send_key("r");
    thread::sleep(Duration::from_millis(BETWEEN_MENU_KEYS_MS));
    send_key("2");
    let ok2_front = wait_for_frontmost_title(&title2, timeout_ms / 2);
    let mut ok2 = if ok2_front {
        true
    } else {
        let ok_hud = rt.block_on(wait_for_title(sess.socket_path(), &title2, timeout_ms / 2));
        if ok_hud {
            true
        } else {
            thread::sleep(Duration::from_millis(RETRY_DELAY_MS));
            send_key("2");
            wait_for_frontmost_title(&title2, timeout_ms / 2)
                || rt.block_on(wait_for_title(sess.socket_path(), &title2, timeout_ms / 2))
        }
    };
    if !ok2 {
        // Final robust attempt for the second window as well
        send_key("shift+cmd+0");
        thread::sleep(Duration::from_millis(OPEN_MENU_STAGGER_MS));
        send_key("r");
        let _ = wait_for_windows(&[(pid2, &title2)], WAIT_WINDOWS_RECHECK_MS);
        send_key("2");
        ok2 = wait_for_frontmost_title(&title2, timeout_ms / 2)
            || rt.block_on(wait_for_title(sess.socket_path(), &title2, timeout_ms / 2));
    }

    if !ok2 {
        return Err(Error::FocusNotObserved {
            timeout_ms,
            expected: title2,
        });
    }
    Ok(())
}
