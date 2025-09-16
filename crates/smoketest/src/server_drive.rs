use std::{
    future::Future,
    result::Result as StdResult,
    sync::OnceLock,
    thread,
    time::{Duration, Instant},
};

use parking_lot::Mutex;
use thiserror::Error;
use tokio::time::timeout;

use crate::{config, error::Error as SmoketestError, runtime};

/// Shared connection slot to the hotki-server.
static CONN: OnceLock<Mutex<Option<hotki_server::Connection>>> = OnceLock::new();
/// Flag ensuring the drain loop starts only once.
static DRAIN_STARTED: OnceLock<()> = OnceLock::new();

/// Result alias for MRPC driver operations.
pub type DriverResult<T> = StdResult<T, DriverError>;

/// Error variants surfaced by the smoketest MRPC driver.
#[derive(Debug, Error)]
pub enum DriverError {
    /// Global runtime was unavailable or failed to execute a future.
    #[error("async runtime failed while {action}: {cause}")]
    RuntimeFailure {
        /// Human-friendly action label.
        action: &'static str,
        /// Original error message stringified to avoid recursive types.
        cause: String,
    },
    /// Connecting to the MRPC socket failed.
    #[error("failed to connect to MRPC socket '{socket_path}': {source}")]
    Connect {
        /// Socket path we attempted to reach.
        socket_path: String,
        /// Underlying hotki-server error.
        #[source]
        source: hotki_server::Error,
    },
    /// A connection-dependent operation was attempted before initialization.
    #[error("MRPC connection not initialized")]
    NotInitialized,
    /// Exhausted retries while waiting for the MRPC connection to become ready.
    #[error("timed out after {timeout_ms} ms initializing MRPC at '{socket_path}': {last_error}")]
    InitTimeout {
        /// Socket path we attempted to reach.
        socket_path: String,
        /// Timeout duration in milliseconds.
        timeout_ms: u64,
        /// Last observed error message.
        last_error: String,
    },
    /// A specific RPC failed even though a connection existed.
    #[error("{action} RPC failed: {source}")]
    RpcFailure {
        /// Which RPC call failed.
        action: &'static str,
        /// Underlying hotki-server error.
        #[source]
        source: hotki_server::Error,
    },
    /// Waiting for a binding to appear timed out.
    #[error("timed out after {timeout_ms} ms waiting for binding '{ident}'")]
    BindingTimeout {
        /// Identifier we were waiting for.
        ident: String,
        /// Timeout duration in milliseconds.
        timeout_ms: u64,
    },
    /// Waiting for a focus PID match timed out.
    #[error("timed out after {timeout_ms} ms waiting for focused pid {expected_pid}")]
    FocusPidTimeout {
        /// Expected process identifier.
        expected_pid: i32,
        /// Timeout duration in milliseconds.
        timeout_ms: u64,
    },
    /// Waiting for a focus title match timed out.
    #[error("timed out after {timeout_ms} ms waiting for focused title '{expected_title}'")]
    FocusTitleTimeout {
        /// Expected window title.
        expected_title: String,
        /// Timeout duration in milliseconds.
        timeout_ms: u64,
    },
}

impl DriverError {
    /// Convert a smoketest runtime error into a driver-specific failure.
    fn runtime(action: &'static str, err: &SmoketestError) -> Self {
        Self::RuntimeFailure {
            action,
            cause: err.to_string(),
        }
    }
}

/// Access the global connection slot, initializing storage if needed.
fn conn_slot() -> &'static Mutex<Option<hotki_server::Connection>> {
    CONN.get_or_init(|| Mutex::new(None))
}

/// Run an async MRPC call on the shared runtime and map errors into driver variants.
fn block_on_rpc<F, T>(action: &'static str, fut: F) -> DriverResult<T>
where
    F: Future<Output = hotki_server::Result<T>>,
{
    runtime::block_on(fut)
        .map_err(|e| DriverError::runtime(action, &e))?
        .map_err(|source| DriverError::RpcFailure { action, source })
}

/// Start the background drain loop if it has not been launched yet.
fn ensure_drain_thread_started() {
    let _ = DRAIN_STARTED.get_or_init(|| {
        thread::spawn(drain_events_loop);
    });
}

/// Borrow the shared connection mutably, returning an error when uninitialized.
fn with_connection_mut<R>(
    f: impl FnOnce(&mut hotki_server::Connection) -> DriverResult<R>,
) -> DriverResult<R> {
    let mut guard = conn_slot().lock();
    let conn = guard.as_mut().ok_or(DriverError::NotInitialized)?;
    f(conn)
}

/// Initialize a shared MRPC connection to the hotki-server at `socket_path`.
pub fn init(socket_path: &str) -> DriverResult<()> {
    if conn_slot().lock().is_some() {
        return Ok(());
    }

    let socket_path_buf = socket_path.to_string();
    match runtime::block_on(async { hotki_server::Connection::connect_unix(socket_path).await }) {
        Ok(Ok(conn)) => {
            let mut guard = conn_slot().lock();
            *guard = Some(conn);
            Ok(())
        }
        Ok(Err(source)) => Err(DriverError::Connect {
            socket_path: socket_path_buf,
            source,
        }),
        Err(err) => Err(DriverError::runtime("connect", &err)),
    }
}

/// Returns true if a connection is available for RPC driving.
pub fn is_ready() -> bool {
    conn_slot().lock().is_some()
}

/// Ensure the shared MRPC connection is initialized within `timeout_ms`.
pub fn ensure_init(socket_path: &str, timeout_ms: u64) -> DriverResult<()> {
    if is_ready() {
        ensure_drain_thread_started();
        return Ok(());
    }

    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    let mut last_error: Option<String> = None;

    while Instant::now() < deadline {
        match init(socket_path) {
            Ok(()) => {
                ensure_drain_thread_started();
                return Ok(());
            }
            Err(err) => {
                last_error = Some(err.to_string());
                thread::sleep(config::ms(config::FAST_RETRY_DELAY_MS));
            }
        }
    }

    Err(DriverError::InitTimeout {
        socket_path: socket_path.to_string(),
        timeout_ms,
        last_error: last_error.unwrap_or_else(|| "no connection attempts were made".to_string()),
    })
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
pub fn inject_key(seq: &str) -> DriverResult<()> {
    let ident = mac_keycode::Chord::parse(seq)
        .map(|c| c.to_string())
        .unwrap_or_else(|| seq.to_string());

    with_connection_mut(|conn| {
        block_on_rpc("inject_key_down", async {
            conn.inject_key_down(&ident).await
        })?;
        thread::sleep(config::ms(config::KEY_EVENT_DELAY_MS));
        match block_on_rpc("inject_key_up", async { conn.inject_key_up(&ident).await }) {
            Ok(_) => {}
            Err(DriverError::RpcFailure { action, source })
                if action == "inject_key_up"
                    && matches!(
                        source,
                        hotki_server::Error::Ipc(ref msg) if msg.contains("KeyNotBound")
                    ) => {}
            Err(err) => return Err(err),
        }
        Ok(())
    })
}

/// Inject a sequence of key presses with UI delays.
pub fn inject_sequence(sequences: &[&str]) -> DriverResult<()> {
    for s in sequences {
        inject_key(s)?;
        thread::sleep(config::ms(config::UI_ACTION_DELAY_MS));
    }
    Ok(())
}

/// Return current bindings if connected.
pub fn get_bindings() -> DriverResult<Vec<String>> {
    with_connection_mut(|conn| block_on_rpc("get_bindings", async { conn.get_bindings().await }))
}

/// Wait until a specific identifier is present in the current bindings.
pub fn wait_for_ident(ident: &str, timeout_ms: u64) -> DriverResult<()> {
    let want = mac_keycode::Chord::parse(ident)
        .map(|c| c.to_string())
        .unwrap_or_else(|| ident.to_string());
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    while Instant::now() < deadline {
        let binds = get_bindings()?;
        if binds.iter().any(|b| {
            let trimmed = b.trim_matches('"');
            let normalized = mac_keycode::Chord::parse(trimmed)
                .map(|c| c.to_string())
                .unwrap_or_else(|| trimmed.to_string());
            normalized == want
        }) {
            return Ok(());
        }
        thread::sleep(config::ms(config::RETRY_DELAY_MS));
    }
    Err(DriverError::BindingTimeout {
        ident: want,
        timeout_ms,
    })
}

/// Quick liveness probe against the backend via a lightweight RPC.
pub fn check_alive() -> DriverResult<()> {
    with_connection_mut(|conn| {
        block_on_rpc("get_depth", async { conn.get_depth().await })?;
        Ok(())
    })
}

/// Fetch a lightweight world snapshot from the backend, if connected.
pub fn get_world_snapshot() -> DriverResult<hotki_server::WorldSnapshotLite> {
    with_connection_mut(|conn| {
        block_on_rpc("get_world_snapshot", async {
            conn.get_world_snapshot().await
        })
    })
}

/// Wait until the backend reports the focused PID equals `expected_pid`.
pub fn wait_for_focused_pid(expected_pid: i32, timeout_ms: u64) -> DriverResult<()> {
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    while Instant::now() < deadline {
        let snap = get_world_snapshot()?;
        if let Some(app) = snap.focused
            && app.pid == expected_pid
        {
            return Ok(());
        }
        thread::sleep(config::ms(config::POLL_INTERVAL_MS));
    }
    Err(DriverError::FocusPidTimeout {
        expected_pid,
        timeout_ms,
    })
}

/// Wait until the backend reports the focused title equals `expected_title`.
pub fn wait_for_focused_title(expected_title: &str, timeout_ms: u64) -> DriverResult<()> {
    let want = expected_title;
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    while Instant::now() < deadline {
        let snap = get_world_snapshot()?;
        if let Some(app) = snap.focused
            && app.title == want
        {
            return Ok(());
        }
        thread::sleep(config::ms(config::POLL_INTERVAL_MS));
    }
    Err(DriverError::FocusTitleTimeout {
        expected_title: want.to_string(),
        timeout_ms,
    })
}

#[cfg(test)]
mod tests {
    use std::{
        process,
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::*;

    fn unique_missing_socket() -> String {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        format!("/tmp/hotki-missing-{}-{}.sock", process::id(), nanos)
    }

    #[test]
    fn inject_key_requires_initialization() {
        reset();
        let err = inject_key("cmd+shift+9").unwrap_err();
        assert!(matches!(err, DriverError::NotInitialized));
    }

    #[test]
    fn ensure_init_times_out_for_missing_socket() {
        reset();
        let path = unique_missing_socket();
        let err = ensure_init(&path, 25).unwrap_err();
        match err {
            DriverError::InitTimeout { socket_path, .. } => assert_eq!(socket_path, path),
            other => panic!("expected InitTimeout, got {:?}", other),
        }
    }

    #[test]
    fn get_bindings_fails_without_connection() {
        reset();
        let err = get_bindings().unwrap_err();
        assert!(matches!(err, DriverError::NotInitialized));
    }

    #[test]
    fn check_alive_without_connection_reports_error() {
        reset();
        let err = check_alive().unwrap_err();
        assert!(matches!(err, DriverError::NotInitialized));
    }
}
