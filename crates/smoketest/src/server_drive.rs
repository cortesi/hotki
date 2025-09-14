use std::{
    sync::OnceLock,
    thread,
    time::{Duration, Instant},
};

use parking_lot::Mutex;
use tokio::time::timeout;

use crate::{config, runtime};

/// Shared connection slot to the hotki-server.
static CONN: OnceLock<Mutex<Option<hotki_server::Connection>>> = OnceLock::new();
/// Flag ensuring the drain loop starts only once.
static DRAIN_STARTED: OnceLock<()> = OnceLock::new();

/// Access the global connection slot, initializing storage if needed.
fn conn_slot() -> &'static Mutex<Option<hotki_server::Connection>> {
    CONN.get_or_init(|| Mutex::new(None))
}

/// Initialize a shared MRPC connection to the hotki-server at `socket_path`.
pub fn init(socket_path: &str) -> bool {
    // If already initialized, avoid reconnecting.
    if conn_slot().lock().is_some() {
        return true;
    }
    let res =
        runtime::block_on(async { hotki_server::Connection::connect_unix(socket_path).await });
    match res {
        Ok(Ok(c)) => {
            let mut g = conn_slot().lock();
            *g = Some(c);
            true
        }
        _ => false,
    }
}

/// Returns true if a connection is available for RPC driving.
pub fn is_ready() -> bool {
    conn_slot().lock().is_some()
}

/// Ensure the shared MRPC connection is initialized within `timeout_ms`.
/// Returns true on success.
pub fn ensure_init(socket_path: &str, timeout_ms: u64) -> bool {
    if is_ready() {
        // Start drain if required and return.
        let _ = DRAIN_STARTED.get_or_init(|| {
            thread::spawn(drain_events_loop);
        });
        return true;
    }
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    let mut inited = init(socket_path);
    while !inited && Instant::now() < deadline {
        thread::sleep(config::ms(config::FAST_RETRY_DELAY_MS));
        inited = init(socket_path);
    }
    if inited {
        // Start a background drain exactly once to keep the event channel alive
        // and avoid closed‑receiver errors from the MRPC client handler.
        let _ = DRAIN_STARTED.get_or_init(|| {
            thread::spawn(drain_events_loop);
        });
    }
    inited
}

/// Drop the shared MRPC connection so subsequent tests start clean.
pub fn reset() {
    let mut g = conn_slot().lock();
    *g = None;
}

/// Background loop that keeps the MRPC event receiver alive by
/// draining notifications opportunistically with a short timeout.
///
/// This avoids the situation where the server sends a heartbeat or log event
/// and the client handler finds the receiver already dropped, which would
/// otherwise log an error. The loop exits when the connection is removed via
/// `reset()` or when the server disconnects.
fn drain_events_loop() {
    use std::time::Duration;
    loop {
        let maybe_ev = {
            let mut guard = conn_slot().lock();
            match guard.as_mut() {
                Some(conn) => {
                    // Poll with a short timeout to avoid holding the lock long.
                    let res = runtime::block_on(async {
                        timeout(Duration::from_millis(40), conn.recv_event()).await
                    });
                    Some(res)
                }
                None => None,
            }
        };
        match maybe_ev {
            None => {
                // No active connection; exit quietly.
                break;
            }
            Some(Ok(Ok(Ok(_msg)))) => {
                // Drained one event; continue.
            }
            Some(Ok(Ok(Err(_)))) => {
                // Channel closed: likely server shutting down; loop again
                // to observe removal via reset().
                thread::sleep(Duration::from_millis(20));
            }
            Some(Ok(Err(_timeout))) => {
                // No event within timeout; yield.
                thread::sleep(Duration::from_millis(20));
            }
            Some(Err(_join_err)) => {
                // Runtime unavailable; yield and retry.
                thread::sleep(Duration::from_millis(20));
            }
        }
    }
}

/// Inject a single key press (down + small delay + up) via MRPC.
pub fn inject_key(seq: &str) -> bool {
    let mut guard = conn_slot().lock();
    let conn = match guard.as_mut() {
        Some(c) => c,
        None => return false,
    };
    // Canonicalize ident to engine format (e.g., cmd+shift+0)
    let ident = mac_keycode::Chord::parse(seq)
        .map(|c| c.to_string())
        .unwrap_or_else(|| seq.to_string());
    // Drive down -> delay -> up
    let ok = runtime::block_on(async { conn.inject_key_down(&ident).await }).is_ok();
    thread::sleep(config::ms(config::KEY_EVENT_DELAY_MS));
    let ok2 = runtime::block_on(async { conn.inject_key_up(&ident).await }).is_ok();
    ok && ok2
}

/// Inject a sequence of key presses with UI delays.
pub fn inject_sequence(sequences: &[&str]) -> bool {
    for s in sequences {
        if !inject_key(s) {
            return false;
        }
        thread::sleep(config::ms(config::UI_ACTION_DELAY_MS));
    }
    true
}

/// Return current bindings if connected.
pub fn get_bindings() -> Option<Vec<String>> {
    let mut guard = conn_slot().lock();
    let conn = guard.as_mut()?;
    match runtime::block_on(async { conn.get_bindings().await }) {
        Ok(Ok(v)) => Some(v),
        _ => None,
    }
}

/// Wait until a specific identifier is present in the current bindings.
pub fn wait_for_ident(ident: &str, timeout_ms: u64) -> bool {
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    while Instant::now() < deadline {
        if let Some(binds) = get_bindings()
            && binds.iter().any(|b| b == ident)
        {
            return true;
        }
        thread::sleep(config::ms(config::RETRY_DELAY_MS));
    }
    false
}

/// Quick liveness probe against the backend via a lightweight RPC.
/// Returns false if the connection is not ready or the RPC fails.
pub fn check_alive() -> bool {
    let mut guard = conn_slot().lock();
    let conn = match guard.as_mut() {
        Some(c) => c,
        None => return false,
    };
    matches!(
        runtime::block_on(async { conn.get_depth().await }),
        Ok(Ok(_))
    )
}

/// Fetch a lightweight world snapshot from the backend, if connected.
pub fn get_world_snapshot() -> Option<hotki_server::WorldSnapshotLite> {
    let mut guard = conn_slot().lock();
    let conn = guard.as_mut()?;
    match runtime::block_on(async { conn.get_world_snapshot().await }) {
        Ok(Ok(s)) => Some(s),
        _ => None,
    }
}

/// Wait until the backend reports the focused PID equals `expected_pid`.
/// Returns true on success within `timeout_ms`.
pub fn wait_for_focused_pid(expected_pid: i32, timeout_ms: u64) -> bool {
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    while Instant::now() < deadline {
        if let Some(snap) = get_world_snapshot()
            && let Some(app) = snap.focused
            && app.pid == expected_pid
        {
            return true;
        }
        thread::sleep(config::ms(config::POLL_INTERVAL_MS));
    }
    false
}
