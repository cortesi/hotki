use std::{
    collections::BTreeSet,
    env,
    io::{self, BufRead, BufReader, Write},
    os::unix::net::UnixStream,
    path::Path,
    sync::OnceLock,
    thread,
    time::{Duration, Instant},
};

use parking_lot::Mutex;
use thiserror::Error;
use tracing::debug;

use crate::config;
use hotki_server::{
    WorldSnapshotLite,
    smoketest_bridge::{BridgeKeyKind, BridgeRequest, BridgeResponse},
};

/// Shared bridge client slot to the UI smoketest bridge.
static CONN: OnceLock<Mutex<Option<BridgeClient>>> = OnceLock::new();
/// Flag to enable verbose binding polling diagnostics.
static LOG_BINDINGS: OnceLock<bool> = OnceLock::new();

/// Return true when verbose binding diagnostics are enabled via env flag.
fn log_bindings_enabled() -> bool {
    *LOG_BINDINGS.get_or_init(|| env::var_os("SMOKETEST_LOG_BINDINGS").is_some())
}

/// Result alias for bridge driver operations.
pub type DriverResult<T> = Result<T, DriverError>;

/// Error variants surfaced by the smoketest bridge driver.
#[derive(Debug, Error)]
pub enum DriverError {
    /// Connecting to the bridge socket failed.
    #[error("failed to connect to bridge socket '{socket_path}': {source}")]
    Connect {
        /// Socket path we attempted to reach.
        socket_path: String,
        /// Underlying IO error.
        #[source]
        source: io::Error,
    },
    /// A bridge command was attempted before initialization.
    #[error("bridge connection not initialized")]
    NotInitialized,
    /// Exhausted retries while waiting for the bridge to become ready.
    #[error("timed out after {timeout_ms} ms initializing bridge at '{socket_path}': {last_error}")]
    InitTimeout {
        /// Socket path we attempted to reach.
        socket_path: String,
        /// Timeout duration in milliseconds.
        timeout_ms: u64,
        /// Last observed error message.
        last_error: String,
    },
    /// Bridge reported a failure while handling a command.
    #[error("bridge command failed: {message}")]
    BridgeFailure {
        /// Human-readable error message from the bridge.
        message: String,
    },
    /// Waiting for a binding to appear timed out.
    #[error("timed out after {timeout_ms} ms waiting for binding '{ident}'")]
    BindingTimeout {
        /// Identifier we were waiting for.
        ident: String,
        /// Timeout duration in milliseconds.
        timeout_ms: u64,
    },
    /// Bridge IO error while sending/receiving commands.
    #[error("bridge I/O error: {source}")]
    Io {
        /// Underlying IO error.
        #[source]
        source: io::Error,
    },
}

/// Normalize an identifier by parsing it as a chord when possible.
fn canonicalize_ident(raw: &str) -> String {
    mac_keycode::Chord::parse(raw)
        .map(|c| c.to_string())
        .unwrap_or_else(|| raw.to_string())
}

/// Access the global connection slot, initializing storage if needed.
fn conn_slot() -> &'static Mutex<Option<BridgeClient>> {
    CONN.get_or_init(|| Mutex::new(None))
}

/// Borrow the global bridge client mutably and execute the provided closure.
fn with_connection_mut<R>(f: impl FnOnce(&mut BridgeClient) -> DriverResult<R>) -> DriverResult<R> {
    let mut guard = conn_slot().lock();
    let conn = guard.as_mut().ok_or(DriverError::NotInitialized)?;
    f(conn)
}

/// Derive the control socket path from the server socket path.
fn control_socket_path(server_socket: &str) -> String {
    format!("{server_socket}.bridge")
}

/// Initialize a shared bridge connection to the UI runtime.
pub fn init(socket_path: &str) -> DriverResult<()> {
    if conn_slot().lock().is_some() {
        return Ok(());
    }

    let control_path = control_socket_path(socket_path);
    let mut client = BridgeClient::connect(&control_path)?;
    client.call_ok(&BridgeRequest::Ping)?;

    let mut guard = conn_slot().lock();
    *guard = Some(client);
    Ok(())
}

/// Returns true if a connection is available for RPC driving.
pub fn is_ready() -> bool {
    conn_slot().lock().is_some()
}

/// Ensure the shared bridge is initialized within `timeout_ms`.
pub fn ensure_init(socket_path: &str, timeout_ms: u64) -> DriverResult<()> {
    if is_ready() {
        return Ok(());
    }

    let control_path = control_socket_path(socket_path);
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    let mut last_error: Option<String> = None;

    while Instant::now() < deadline {
        match init(socket_path) {
            Ok(()) => return Ok(()),
            Err(err) => {
                last_error = Some(match &err {
                    DriverError::Connect { source, .. } => source.to_string(),
                    DriverError::BridgeFailure { message } => message.clone(),
                    DriverError::Io { source } => source.to_string(),
                    other => other.to_string(),
                });
                thread::sleep(config::ms(config::RETRY.fast_delay_ms));
            }
        }
    }

    Err(DriverError::InitTimeout {
        socket_path: control_path,
        timeout_ms,
        last_error: last_error.unwrap_or_else(|| "no connection attempts were made".to_string()),
    })
}

/// Request a graceful shutdown via the active bridge connection, if available.
pub fn shutdown() -> DriverResult<()> {
    match with_connection_mut(|conn| conn.call_ok(&BridgeRequest::Shutdown)) {
        Err(DriverError::NotInitialized) => Ok(()),
        Err(err) => {
            reset();
            Err(err)
        }
        Ok(()) => {
            reset();
            Ok(())
        }
    }
}

/// Drop the shared bridge connection so subsequent tests start clean.
pub fn reset() {
    let mut g = conn_slot().lock();
    *g = None;
}

/// Inject a single key press (down + small delay + up) via the bridge.
pub fn inject_key(seq: &str) -> DriverResult<()> {
    let ident = canonicalize_ident(seq);
    let gate_ms = config::BINDING_GATES.default_ms.saturating_mul(3);
    let deadline = Instant::now() + Duration::from_millis(gate_ms);

    loop {
        match inject_key_down_once(&ident) {
            Ok(()) => break,
            Err(DriverError::BridgeFailure { message })
                if message_contains_key_not_bound(&message) =>
            {
                if Instant::now() >= deadline {
                    return Err(DriverError::BindingTimeout {
                        ident: ident.clone(),
                        timeout_ms: gate_ms,
                    });
                }
                thread::sleep(config::ms(config::INPUT_DELAYS.retry_delay_ms));
            }
            Err(err) => return Err(err),
        }
    }

    thread::sleep(config::ms(config::INPUT_DELAYS.key_event_delay_ms));

    match inject_key_up_once(&ident) {
        Ok(()) => {}
        Err(DriverError::BridgeFailure { message }) if message_contains_key_not_bound(&message) => {
        }
        Err(err) => return Err(err),
    }

    Ok(())
}

/// Issue a single key-down event via the bridge without retries.
fn inject_key_down_once(ident: &str) -> DriverResult<()> {
    with_connection_mut(|conn| {
        conn.call_ok(&BridgeRequest::InjectKey {
            ident: ident.to_string(),
            kind: BridgeKeyKind::Down,
            repeat: false,
        })
    })
}

/// Issue a single key-up event via the bridge without retries.
fn inject_key_up_once(ident: &str) -> DriverResult<()> {
    with_connection_mut(|conn| {
        conn.call_ok(&BridgeRequest::InjectKey {
            ident: ident.to_string(),
            kind: BridgeKeyKind::Up,
            repeat: false,
        })
    })
}

/// Returns true when the bridge error message indicates a missing key binding.
fn message_contains_key_not_bound(msg: &str) -> bool {
    msg.contains("KeyNotBound")
}

/// Inject a sequence of key presses with UI delays.
pub fn inject_sequence(sequences: &[&str]) -> DriverResult<()> {
    for s in sequences {
        inject_key(s)?;
        thread::sleep(config::ms(config::INPUT_DELAYS.ui_action_delay_ms));
    }
    Ok(())
}

/// Return current bindings if connected.
pub fn get_bindings() -> DriverResult<Vec<String>> {
    with_connection_mut(|conn| conn.call_bindings())
}

/// Load a configuration from disk and apply it to the running server.
pub fn set_config_from_path(path: &Path) -> DriverResult<()> {
    let path_str = path.to_str().ok_or_else(|| DriverError::BridgeFailure {
        message: format!("non-UTF-8 config path: {}", path.display()),
    })?;
    with_connection_mut(|conn| {
        conn.call_ok(&BridgeRequest::SetConfig {
            path: path_str.to_string(),
        })
    })
}

/// Wait until all identifiers are present in the current bindings.
pub fn wait_for_idents(idents: &[&str], timeout_ms: u64) -> DriverResult<()> {
    if idents.is_empty() {
        return Ok(());
    }

    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    let mut remaining: BTreeSet<String> = idents
        .iter()
        .map(|ident| canonicalize_ident(ident))
        .collect();
    let mut last_snapshot: Vec<String> = Vec::new();
    let start = Instant::now();

    while Instant::now() < deadline {
        let binds = get_bindings()?;
        let mut snapshot = Vec::with_capacity(binds.len());
        for binding in binds {
            let trimmed = binding.trim_matches('"');
            let normalized = canonicalize_ident(trimmed);
            snapshot.push(normalized.clone());
            remaining.remove(&normalized);
        }

        if log_bindings_enabled() {
            let elapsed_ms = start.elapsed().as_millis() as u64;
            let remaining_list = remaining.iter().cloned().collect::<Vec<_>>();
            debug!(
                elapsed_ms,
                snapshot = ?snapshot,
                remaining = ?remaining_list,
                "wait_for_idents_poll"
            );
        }

        last_snapshot = snapshot;

        if remaining.is_empty() {
            return Ok(());
        }

        thread::sleep(config::ms(config::INPUT_DELAYS.retry_delay_ms));
    }

    let missing = remaining.into_iter().collect::<Vec<_>>().join(", ");
    if log_bindings_enabled() {
        let elapsed_ms = start.elapsed().as_millis() as u64;
        debug!(
            elapsed_ms,
            snapshot = ?last_snapshot,
            missing = %missing,
            "wait_for_idents_timeout"
        );
    }
    Err(DriverError::BindingTimeout {
        ident: missing,
        timeout_ms,
    })
}

/// Quick liveness probe against the backend via a lightweight bridge command.
pub fn check_alive() -> DriverResult<()> {
    with_connection_mut(|conn| conn.call_depth().map(|_| ()))
}

/// Fetch a lightweight world snapshot from the backend, if connected.
pub fn get_world_snapshot() -> DriverResult<WorldSnapshotLite> {
    with_connection_mut(|conn| conn.call_snapshot())
}

/// Blocking Unix-stream client that forwards commands to the UI bridge.
struct BridgeClient {
    /// Reader half of the bridge socket.
    reader: BufReader<UnixStream>,
    /// Writer half of the bridge socket.
    writer: UnixStream,
    /// Path to the bridge socket, used for diagnostics.
    socket_path: String,
}

impl BridgeClient {
    /// Establish a new bridge client connection to the given socket path.
    fn connect(path: &str) -> DriverResult<Self> {
        let writer = UnixStream::connect(path).map_err(|source| DriverError::Connect {
            socket_path: path.to_string(),
            source,
        })?;
        writer.set_nonblocking(false).ok();
        let reader_stream = writer
            .try_clone()
            .map_err(|source| DriverError::Io { source })?;
        Ok(Self {
            reader: BufReader::new(reader_stream),
            writer,
            socket_path: path.to_string(),
        })
    }

    /// Send a bridge request and wait for its response.
    fn call(&mut self, req: &BridgeRequest) -> DriverResult<BridgeResponse> {
        let json = serde_json::to_string(req).map_err(|err| DriverError::BridgeFailure {
            message: err.to_string(),
        })?;
        self.writer
            .write_all(json.as_bytes())
            .map_err(|source| DriverError::Io { source })?;
        self.writer
            .write_all(b"\n")
            .map_err(|source| DriverError::Io { source })?;
        self.writer
            .flush()
            .map_err(|source| DriverError::Io { source })?;

        let mut line = String::new();
        let bytes = self
            .reader
            .read_line(&mut line)
            .map_err(|source| DriverError::Io { source })?;
        if bytes == 0 {
            return Err(DriverError::BridgeFailure {
                message: format!("bridge socket '{}' closed", self.socket_path),
            });
        }
        let trimmed = line.trim_end_matches(['\n', '\r']);
        serde_json::from_str(trimmed).map_err(|err| DriverError::BridgeFailure {
            message: err.to_string(),
        })
    }

    /// Send a bridge request that is expected to return `BridgeResponse::Ok`.
    fn call_ok(&mut self, req: &BridgeRequest) -> DriverResult<()> {
        self.call(req)?
            .into_result()
            .map_err(|message| DriverError::BridgeFailure { message })
    }

    /// Retrieve the current bindings list via the bridge.
    fn call_bindings(&mut self) -> DriverResult<Vec<String>> {
        self.call(&BridgeRequest::GetBindings)?
            .into_bindings()
            .map_err(|message| DriverError::BridgeFailure { message })
    }

    /// Retrieve the current depth value via the bridge.
    fn call_depth(&mut self) -> DriverResult<usize> {
        self.call(&BridgeRequest::GetDepth)?
            .into_depth()
            .map_err(|message| DriverError::BridgeFailure { message })
    }

    /// Retrieve a `WorldSnapshotLite` via the bridge.
    fn call_snapshot(&mut self) -> DriverResult<WorldSnapshotLite> {
        self.call(&BridgeRequest::GetWorldSnapshot)?
            .into_snapshot()
            .map_err(|message| DriverError::BridgeFailure { message })
    }
}

#[cfg(test)]
mod tests {
    use std::{
        process,
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::*;

    fn unique_control_socket() -> String {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        format!("/tmp/hotki-bridge-test-{}-{}.sock", process::id(), nanos)
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
        let path = unique_control_socket();
        let err = ensure_init(&path, 25).unwrap_err();
        match err {
            DriverError::InitTimeout { socket_path, .. } => {
                assert_eq!(socket_path, control_socket_path(&path))
            }
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

    #[test]
    fn control_socket_path_appends_suffix() {
        let path = "/tmp/hotki.sock";
        assert_eq!(control_socket_path(path), "/tmp/hotki.sock.bridge");
    }
}
