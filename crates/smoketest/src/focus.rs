use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use crate::{
    config,
    error::{Error, Result},
    process::HelperWindowBuilder,
    results::FocusOutcome,
    test_runner::{TestConfig, TestRunner},
};


/// Listen for focus events on the given socket
async fn listen_for_focus(
    socket_path: &str,
    expected_title: String,
    found: Arc<AtomicBool>,
    done: Arc<AtomicBool>,
    matched: Arc<Mutex<Option<(String, i32)>>>,
) {
    let mut client = match hotki_server::Client::new_with_socket(socket_path)
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

    let per_wait = config::ms(config::EVENT_CHECK_INTERVAL_MS);
    loop {
        if done.load(Ordering::SeqCst) {
            break;
        }
        
        let res = tokio::time::timeout(per_wait, conn.recv_event()).await;
        match res {
            Ok(Ok(hotki_protocol::MsgToUI::HudUpdate { cursor })) => {
                if let Some(app) = cursor.app_ref() {
                    if app.title == expected_title {
                        if let Ok(mut g) = matched.lock() {
                            *g = Some((app.title.clone(), app.pid));
                        }
                        found.store(true, Ordering::SeqCst);
                        break;
                    }
                }
            }
            Ok(Ok(_)) => {}
            Ok(Err(_)) => break,
            Err(_) => {}
        }
    }
}

pub(crate) fn run_focus_test(timeout_ms: u64, with_logs: bool) -> Result<FocusOutcome> {
    let config = TestConfig::new(timeout_ms)
        .with_logs(with_logs);

    // Generate unique title for the test window
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let expected_title = config::focus_test_title(unique);
    
    // Shared state for event listener
    let found = Arc::new(AtomicBool::new(false));
    let done = Arc::new(AtomicBool::new(false));
    let matched: Arc<Mutex<Option<(String, i32)>>> = Arc::new(Mutex::new(None));
    
    TestRunner::new("focus_test", config)
        .with_setup(|ctx| {
            ctx.launch_hotki()?;
            Ok(())
        })
        .with_execute(move |ctx| {
            // Get socket path from session
            let socket_path = ctx.session.as_ref()
                .ok_or_else(|| Error::InvalidState("No session".into()))?
                .socket_path()
                .to_string();
            
            // Start background listener
            let expected_title_clone = expected_title.clone();
            let found_clone = found.clone();
            let done_clone = done.clone();
            let matched_clone = matched.clone();
            
            let listener = thread::spawn(move || {
                let rt = match tokio::runtime::Runtime::new() {
                    Ok(rt) => rt,
                    Err(_) => return,
                };
                rt.block_on(listen_for_focus(
                    &socket_path,
                    expected_title_clone,
                    found_clone,
                    done_clone,
                    matched_clone,
                ));
            });
            
            // Spawn helper window
            let helper_time = timeout_ms.saturating_add(config::HELPER_WINDOW_EXTRA_TIME_MS);
            let helper = HelperWindowBuilder::new(expected_title.clone())
                .with_time_ms(helper_time)
                .spawn()?;
            let expected_pid = helper.pid;
            
            // Wait for match or timeout
            let deadline = Instant::now() + Duration::from_millis(timeout_ms);
            let start = Instant::now();
            
            while Instant::now() < deadline {
                if found.load(Ordering::SeqCst) {
                    break;
                }
                thread::sleep(config::ms(config::POLL_INTERVAL_MS));
            }
            
            // Signal listener to stop
            done.store(true, Ordering::SeqCst);
            let _ = listener.join();
            
            // Check if we found the window
            if !found.load(Ordering::SeqCst) {
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