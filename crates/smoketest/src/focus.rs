use std::{
    env,
    process::{Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use crate::{
    error::{Error, Result},
    session::HotkiSession,
    util::resolve_hotki_bin,
};

pub(crate) struct FocusOutcome {
    pub title: String,
    pub pid: i32,
    pub elapsed_ms: u64,
}

pub(crate) fn run_focus_test(timeout_ms: u64, with_logs: bool) -> Result<FocusOutcome> {
    let cwd = env::current_dir()?;
    let cfg_path = cwd.join("examples/test.ron");
    if !cfg_path.exists() {
        return Err(Error::MissingConfig(cfg_path));
    }
    let Some(hotki_bin) = resolve_hotki_bin() else {
        return Err(Error::HotkiBinNotFound);
    };

    let mut sess = HotkiSession::launch_with_config(&hotki_bin, &cfg_path, with_logs)?;

    // Unique title for helper window; pid match will target the helper process
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let expected_title = format!("hotki smoketest: focus {}-{}", std::process::id(), unique);

    let found = Arc::new(AtomicBool::new(false));
    let done = Arc::new(AtomicBool::new(false));
    let matched: Arc<Mutex<Option<(String, i32)>>> = Arc::new(Mutex::new(None));

    // Background listener for HudUpdate events
    let sock = sess.socket_path().to_string();
    let expected_title_clone = expected_title.clone();
    let found_clone = found.clone();
    let done_clone = done.clone();
    let matched_clone = matched.clone();
    let listener = thread::spawn(move || {
        let rt = match tokio::runtime::Runtime::new() {
            Ok(rt) => rt,
            Err(_) => return,
        };
        rt.block_on(async move {
            let mut client = match hotki_server::Client::new_with_socket(&sock)
                .with_connect_only()
                .connect()
                .await
            {
                Ok(c) => c,
                Err(_) => return,
            };
            let conn = match client.connection() {
                Ok(c) => c,
                Err(_) => return,
            };

            let per_wait = Duration::from_millis(300);
            loop {
                if done_clone.load(Ordering::SeqCst) {
                    break;
                }
                let res = tokio::time::timeout(per_wait, conn.recv_event()).await;
                match res {
                    Ok(Ok(hotki_protocol::MsgToUI::HudUpdate { cursor })) => {
                        if let Some(app) = cursor.app_ref()
                            && app.title == expected_title_clone
                        {
                            if let Ok(mut g) = matched_clone.lock() {
                                *g = Some((app.title.clone(), app.pid));
                            }
                            found_clone.store(true, Ordering::SeqCst);
                            break;
                        }
                    }
                    Ok(Ok(_)) => {}
                    Ok(Err(_)) => break,
                    Err(_) => {}
                }
            }
        });
    });

    // Spawn focus window helper as a separate process to avoid a second EventLoop
    let helper_time = timeout_ms.saturating_add(5000);
    let current_exe = env::current_exe()?;
    let mut child = Command::new(current_exe)
        .arg("focus-winhelper")
        .arg("--title")
        .arg(&expected_title)
        .arg("--time")
        .arg(helper_time.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| Error::SpawnFailed(e.to_string()))?;
    let expected_pid = child.id() as i32;

    // Wait for match or timeout
    let deadline = std::time::Instant::now() + Duration::from_millis(timeout_ms);
    while std::time::Instant::now() < deadline {
        if found.load(Ordering::SeqCst) {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    done.store(true, Ordering::SeqCst);
    let _ = listener.join();

    // Cleanup helper and server
    let _ = child.kill();
    let _ = child.wait();
    sess.shutdown();
    sess.kill_and_wait();

    if !found.load(Ordering::SeqCst) {
        return Err(Error::FocusNotObserved {
            timeout_ms,
            expected: expected_title,
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
        elapsed_ms: 0,
    })
}
