use std::{
    collections::{BTreeSet, VecDeque},
    convert::TryInto,
    env,
    io::{self, BufRead, BufReader, Write},
    os::unix::net::UnixStream,
    path::Path,
    sync::OnceLock,
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use hotki_protocol::{App, Cursor};
use hotki_server::{
    WorldSnapshotLite,
    smoketest_bridge::{
        BridgeCommand, BridgeCommandId, BridgeEvent, BridgeHudKey, BridgeIdleTimerState,
        BridgeKeyKind, BridgeNotification, BridgeReply, BridgeRequest, BridgeResponse,
        BridgeTimestampMs,
    },
};
use parking_lot::Mutex;
use thiserror::Error;
use tracing::debug;

use crate::config;

/// Shared bridge client slot to the UI smoketest bridge.
static CONN: OnceLock<Mutex<Option<BridgeClient>>> = OnceLock::new();
/// Flag to enable verbose binding polling diagnostics.
static LOG_BINDINGS: OnceLock<bool> = OnceLock::new();

/// Return true when verbose binding diagnostics are enabled via env flag.
fn log_bindings_enabled() -> bool {
    *LOG_BINDINGS.get_or_init(|| env::var_os("SMOKETEST_LOG_BINDINGS").is_some())
}

/// Validate invariants returned by the bridge handshake before running tests.
fn ensure_clean_handshake(handshake: &BridgeHandshake) -> DriverResult<()> {
    if handshake.idle_timer.armed {
        return Err(DriverError::BridgeFailure {
            message: format!(
                "server idle timer armed during handshake (deadline_ms={:?})",
                handshake.idle_timer.deadline_ms
            ),
        });
    }
    if handshake.idle_timer.clients_connected == 0 {
        return Err(DriverError::BridgeFailure {
            message: "server reported zero connected clients during handshake".to_string(),
        });
    }
    if let Some(sample) = handshake.notifications.first() {
        return Err(DriverError::BridgeFailure {
            message: format!(
                "bridge reported {} pending notifications, starting with '{}': {}",
                handshake.notifications.len(),
                sample.title,
                sample.text
            ),
        });
    }
    Ok(())
}

/// Result alias for bridge driver operations.
pub type DriverResult<T> = Result<T, DriverError>;

/// Raw bridge event record captured from the UI runtime stream.
#[derive(Debug, Clone)]
pub struct BridgeEventRecord {
    /// Command identifier assigned to the streamed event.
    pub id: BridgeCommandId,
    /// Millisecond timestamp recorded when the UI flushed the event.
    pub timestamp_ms: BridgeTimestampMs,
    /// Event payload describing the state change.
    pub payload: BridgeEvent,
}

/// Snapshot of the most recent HUD update observed on the bridge stream.
#[derive(Debug, Clone)]
pub struct HudSnapshot {
    /// Identifier of the bridge event associated with this snapshot.
    pub event_id: BridgeCommandId,
    /// Millisecond timestamp when the snapshot was observed.
    pub received_ms: BridgeTimestampMs,
    /// Cursor context backing the HUD rendering.
    pub cursor: Cursor,
    /// Logical depth of the HUD stack for the cursor.
    pub depth: usize,
    /// Optional parent title when the HUD is nested.
    pub parent_title: Option<String>,
    /// Keys rendered by the HUD for the current cursor.
    pub keys: Vec<BridgeHudKey>,
}

/// Snapshot of the most recent world focus event observed on the bridge stream.
#[derive(Debug, Clone)]
pub struct WorldFocusSnapshot {
    /// Identifier of the bridge event associated with this focus change.
    pub event_id: BridgeCommandId,
    /// Millisecond timestamp when the focus event was observed.
    pub received_ms: BridgeTimestampMs,
    /// Optional focused application context reported by the world service.
    pub app: Option<App>,
    /// World reconcile sequence at which the focus change occurred.
    pub reconcile_seq: u64,
}

/// Handshake payload returned when the smoketest bridge establishes a session.
#[derive(Debug, Clone)]
pub struct BridgeHandshake {
    /// Idle timer snapshot reported by the UI runtime.
    pub idle_timer: BridgeIdleTimerState,
    /// Pending notifications surfaced by the UI.
    pub notifications: Vec<BridgeNotification>,
}

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
    /// The bridge did not acknowledge a command fast enough.
    #[error("bridge acknowledgement for command {command_id} timed out after {timeout_ms} ms")]
    AckTimeout {
        /// Command identifier we waited on.
        command_id: BridgeCommandId,
        /// Timeout budget that was exceeded in milliseconds.
        timeout_ms: u64,
    },
    /// Bridge responses arrived out of sequence.
    #[error("bridge sequence mismatch: expected command {expected}, got {got}")]
    SequenceMismatch {
        /// Command identifier we expected.
        expected: BridgeCommandId,
        /// Command identifier we observed.
        got: BridgeCommandId,
    },
    /// Bridge failed to emit an acknowledgement before responding.
    #[error("bridge missing ACK for command {command_id}")]
    AckMissing {
        /// Command identifier lacking an acknowledgement.
        command_id: BridgeCommandId,
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
    /// Bridge produced additional messages after shutdown was acknowledged.
    #[error("unexpected bridge message after shutdown: {message}")]
    PostShutdownMessage {
        /// Raw message payload observed.
        message: String,
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
    if let Some(path) = env::var_os("HOTKI_CONTROL_SOCKET")
        && let Some(value) = path.to_str()
    {
        return value.to_string();
    }
    format!("{server_socket}.bridge")
}

/// Initialize a shared bridge connection to the UI runtime.
pub fn init(socket_path: &str) -> DriverResult<()> {
    if conn_slot().lock().is_some() {
        return Ok(());
    }

    let control_path = control_socket_path(socket_path);
    let mut client = BridgeClient::connect(&control_path)?;
    let handshake = client.handshake()?;
    ensure_clean_handshake(&handshake)?;

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
    match with_connection_mut(|conn| {
        let baseline = conn.event_buffer_len();
        conn.call_ok(&BridgeRequest::Shutdown)?;
        conn.assert_no_new_events_since(baseline)
    }) {
        Err(err @ DriverError::NotInitialized) => Err(err),
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

/// Block until the world reconcile sequence reaches `target` (or times out).
pub fn wait_for_world_seq(target: u64, timeout_ms: u64) -> DriverResult<u64> {
    with_connection_mut(|conn| conn.wait_for_world_seq(target, timeout_ms))
}

/// Retrieve the latest HUD snapshot observed on the bridge.
pub fn latest_hud() -> DriverResult<Option<HudSnapshot>> {
    with_connection_mut(|conn| Ok(conn.latest_hud()))
}

/// Retrieve the latest world focus snapshot observed on the bridge.
pub fn latest_world_focus() -> DriverResult<Option<WorldFocusSnapshot>> {
    with_connection_mut(|conn| Ok(conn.latest_focus()))
}

/// Drain buffered bridge events for inspection.
pub fn drain_bridge_events() -> DriverResult<Vec<BridgeEventRecord>> {
    with_connection_mut(|conn| Ok(conn.drain_events()))
}

/// Blocking Unix-stream client that forwards commands to the UI bridge.
struct BridgeClient {
    /// Reader half of the bridge socket.
    reader: BufReader<UnixStream>,
    /// Writer half of the bridge socket.
    writer: UnixStream,
    /// Path to the bridge socket, used for diagnostics.
    socket_path: String,
    /// Next command identifier to allocate.
    next_command_id: BridgeCommandId,
    /// Maximum time to wait for an acknowledgement.
    ack_timeout: Duration,
    /// Circular buffer of recent bridge events.
    event_buffer: VecDeque<BridgeEventRecord>,
    /// Latest HUD snapshot emitted by the bridge.
    latest_hud: Option<HudSnapshot>,
    /// Latest world focus snapshot emitted by the bridge.
    latest_focus: Option<WorldFocusSnapshot>,
    /// Most recent handshake data captured during initialization.
    handshake: Option<BridgeHandshake>,
}

impl BridgeClient {
    /// Maximum number of reconnection attempts per bridge call.
    const MAX_RECONNECT_ATTEMPTS: u32 = 3;
    /// Maximum number of bridge events retained in memory.
    const EVENT_BUFFER_CAPACITY: usize = 128;

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
            next_command_id: 0,
            ack_timeout: Duration::from_millis(config::BRIDGE.ack_timeout_ms),
            event_buffer: VecDeque::new(),
            latest_hud: None,
            latest_focus: None,
            handshake: None,
        })
    }

    /// Perform the bridge handshake and cache the resulting snapshot.
    fn handshake(&mut self) -> DriverResult<BridgeHandshake> {
        match self.call(&BridgeRequest::Ping)? {
            BridgeResponse::Handshake {
                idle_timer,
                notifications,
            } => {
                let payload = BridgeHandshake {
                    idle_timer,
                    notifications,
                };
                self.handshake = Some(payload.clone());
                Ok(payload)
            }
            BridgeResponse::Err { message } => Err(DriverError::BridgeFailure { message }),
            other => Err(DriverError::BridgeFailure {
                message: format!("unexpected handshake response: {:?}", other),
            }),
        }
    }

    /// Send a bridge request and wait for its response.
    fn call(&mut self, req: &BridgeRequest) -> DriverResult<BridgeResponse> {
        let request = req.clone();
        let mut attempt = 0;
        loop {
            let command_id = self.next_command_id;
            let command = BridgeCommand {
                command_id,
                issued_at_ms: now_millis(),
                request: request.clone(),
            };

            match self.send_command(&command) {
                Ok(()) => {}
                Err(DriverError::Io { source })
                    if connection_lost(&source) && attempt < Self::MAX_RECONNECT_ATTEMPTS =>
                {
                    attempt += 1;
                    self.reconnect_with_backoff(attempt)?;
                    continue;
                }
                Err(err @ DriverError::Io { .. }) => return Err(err),
                Err(err) => return Err(err),
            }

            let (acked, result) = self.await_ack_and_response(command_id);
            match result {
                Ok(resp) => {
                    if acked {
                        self.bump_command_id();
                    }
                    return Ok(resp);
                }
                Err(err @ DriverError::BridgeFailure { .. }) if acked => {
                    self.bump_command_id();
                    return Err(err);
                }
                Err(DriverError::Io { source })
                    if connection_lost(&source) && attempt < Self::MAX_RECONNECT_ATTEMPTS =>
                {
                    attempt += 1;
                    self.reconnect_with_backoff(attempt)?;
                    continue;
                }
                Err(err) => return Err(err),
            }
        }
    }

    /// Advance to the next command identifier.
    fn bump_command_id(&mut self) {
        self.next_command_id = self.next_command_id.wrapping_add(1);
    }

    /// Serialize and dispatch a command to the bridge socket.
    fn send_command(&mut self, command: &BridgeCommand) -> DriverResult<()> {
        let encoded = serde_json::to_string(command).map_err(|err| DriverError::BridgeFailure {
            message: err.to_string(),
        })?;
        self.writer
            .write_all(encoded.as_bytes())
            .map_err(|source| DriverError::Io { source })?;
        self.writer
            .write_all(b"\n")
            .map_err(|source| DriverError::Io { source })?;
        self.writer
            .flush()
            .map_err(|source| DriverError::Io { source })
    }

    /// Wait for the bridge to acknowledge the command and provide the final response.
    /// Returns whether the acknowledgement was accepted along with the outcome.
    fn await_ack_and_response(
        &mut self,
        command_id: BridgeCommandId,
    ) -> (bool, DriverResult<BridgeResponse>) {
        if let Err(source) = self
            .reader
            .get_ref()
            .set_read_timeout(Some(self.ack_timeout))
        {
            return (false, Err(DriverError::Io { source }));
        }
        loop {
            let ack_result = self.read_reply();
            match ack_result {
                Ok(reply) => {
                    if let BridgeResponse::Event { .. } = &reply.response {
                        self.record_event(reply);
                        continue;
                    }
                    let outcome = self.validate_ack(command_id, &reply);
                    if let Err(err) = self.reader.get_ref().set_read_timeout(None) {
                        debug!(?err, "failed to clear bridge read timeout");
                    }
                    match outcome {
                        Ok(()) => return (true, self.await_final_response(command_id)),
                        Err(err) => return (false, Err(err)),
                    }
                }
                Err(DriverError::Io { source })
                    if source.kind() == io::ErrorKind::WouldBlock
                        || source.kind() == io::ErrorKind::TimedOut =>
                {
                    if let Err(err) = self.reader.get_ref().set_read_timeout(None) {
                        debug!(?err, "failed to clear bridge read timeout");
                    }
                    return (
                        false,
                        Err(DriverError::AckTimeout {
                            command_id,
                            timeout_ms: self.ack_timeout.as_millis() as u64,
                        }),
                    );
                }
                Err(err) => {
                    if let Err(clear_err) = self.reader.get_ref().set_read_timeout(None) {
                        debug!(?clear_err, "failed to clear bridge read timeout");
                    }
                    return (false, Err(err));
                }
            }
        }
    }

    /// Validate that the acknowledgement matches the expected command id.
    fn validate_ack(&self, command_id: BridgeCommandId, ack: &BridgeReply) -> DriverResult<()> {
        if ack.command_id != command_id {
            return Err(DriverError::SequenceMismatch {
                expected: command_id,
                got: ack.command_id,
            });
        }
        match &ack.response {
            BridgeResponse::Ack { queued } => {
                debug!(command_id, queued, "bridge_ack");
                Ok(())
            }
            BridgeResponse::Err { message } => Err(DriverError::BridgeFailure {
                message: message.clone(),
            }),
            _ => Err(DriverError::AckMissing { command_id }),
        }
    }

    /// Read the final response frame for the supplied command id.
    fn await_final_response(
        &mut self,
        command_id: BridgeCommandId,
    ) -> DriverResult<BridgeResponse> {
        loop {
            let reply = self.read_reply()?;
            if let BridgeResponse::Event { .. } = &reply.response {
                self.record_event(reply);
                continue;
            }
            if reply.command_id != command_id {
                return Err(DriverError::SequenceMismatch {
                    expected: command_id,
                    got: reply.command_id,
                });
            }
            return match reply.response {
                BridgeResponse::Ack { .. } => Err(DriverError::AckMissing { command_id }),
                BridgeResponse::Err { message } => Err(DriverError::BridgeFailure { message }),
                other => Ok(other),
            };
        }
    }

    /// Read and deserialize the next reply frame from the bridge.
    fn read_reply(&mut self) -> DriverResult<BridgeReply> {
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

    /// Record an asynchronous event emitted by the bridge.
    fn record_event(&mut self, reply: BridgeReply) {
        if let BridgeResponse::Event { event } = reply.response {
            if self.event_buffer.len() >= Self::EVENT_BUFFER_CAPACITY {
                self.event_buffer.pop_front();
            }
            match &event {
                BridgeEvent::Hud {
                    cursor,
                    depth,
                    parent_title,
                    keys,
                } => {
                    self.latest_hud = Some(HudSnapshot {
                        event_id: reply.command_id,
                        received_ms: reply.timestamp_ms,
                        cursor: cursor.clone(),
                        depth: *depth,
                        parent_title: parent_title.clone(),
                        keys: keys.clone(),
                    });
                }
                BridgeEvent::WorldFocus { app, reconcile_seq } => {
                    self.latest_focus = Some(WorldFocusSnapshot {
                        event_id: reply.command_id,
                        received_ms: reply.timestamp_ms,
                        app: app.clone(),
                        reconcile_seq: *reconcile_seq,
                    });
                }
            }
            self.event_buffer.push_back(BridgeEventRecord {
                id: reply.command_id,
                timestamp_ms: reply.timestamp_ms,
                payload: event,
            });
        }
    }

    /// Drain the buffered bridge events.
    fn drain_events(&mut self) -> Vec<BridgeEventRecord> {
        self.event_buffer.drain(..).collect()
    }

    /// Return the number of events observed so far.
    fn event_buffer_len(&self) -> usize {
        self.event_buffer.len()
    }

    /// Ensure no additional events arrived after a baseline index.
    fn assert_no_new_events_since(&self, baseline: usize) -> DriverResult<()> {
        if let Some(event) = self.event_buffer.get(baseline) {
            return Err(DriverError::PostShutdownMessage {
                message: format!("bridge event observed after shutdown: {:?}", event.payload),
            });
        }
        Ok(())
    }

    /// Access the latest HUD snapshot observed on the bridge.
    fn latest_hud(&self) -> Option<HudSnapshot> {
        self.latest_hud.clone()
    }

    /// Access the latest world focus snapshot observed on the bridge.
    fn latest_focus(&self) -> Option<WorldFocusSnapshot> {
        self.latest_focus.clone()
    }

    /// Wait for the world reconcile sequence to reach `target` and return the observed value.
    fn wait_for_world_seq(&mut self, target: u64, timeout_ms: u64) -> DriverResult<u64> {
        self.call(&BridgeRequest::WaitForWorldSeq { target, timeout_ms })?
            .into_world_seq()
            .map_err(|message| DriverError::BridgeFailure { message })
    }

    /// Attempt to re-establish the bridge connection with exponential backoff.
    fn reconnect_with_backoff(&mut self, attempt: u32) -> DriverResult<()> {
        let mut last_err: Option<io::Error> = None;
        let mut backoff_ms = config::RETRY.fast_delay_ms.saturating_mul(attempt as u64);
        let max_steps = 3;
        for _ in 0..max_steps {
            thread::sleep(Duration::from_millis(backoff_ms.max(1)));
            match UnixStream::connect(&self.socket_path) {
                Ok(writer) => {
                    writer.set_nonblocking(false).ok();
                    let reader_stream = writer
                        .try_clone()
                        .map_err(|source| DriverError::Io { source })?;
                    self.reader = BufReader::new(reader_stream);
                    self.writer = writer;
                    self.next_command_id = 0;
                    return Ok(());
                }
                Err(err) => {
                    last_err = Some(err);
                    backoff_ms = backoff_ms.saturating_mul(2);
                }
            }
        }
        let source = last_err.unwrap_or_else(|| {
            io::Error::new(
                io::ErrorKind::BrokenPipe,
                "bridge reconnect attempts exhausted",
            )
        });
        Err(DriverError::Connect {
            socket_path: self.socket_path.clone(),
            source,
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

/// Return true when the provided I/O error indicates that the bridge connection dropped.
fn connection_lost(err: &io::Error) -> bool {
    matches!(
        err.kind(),
        io::ErrorKind::BrokenPipe
            | io::ErrorKind::ConnectionReset
            | io::ErrorKind::ConnectionAborted
    )
}

/// Return the current wall-clock timestamp in milliseconds since the Unix epoch.
fn now_millis() -> BridgeTimestampMs {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_else(|_| Duration::from_secs(0));
    duration.as_millis().try_into().unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use std::{
        env, process,
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
        let key = "HOTKI_CONTROL_SOCKET";
        let restore = env::var_os(key);
        unsafe {
            env::remove_var(key);
        }
        let path = "/tmp/hotki.sock";
        assert_eq!(control_socket_path(path), "/tmp/hotki.sock.bridge");
        match restore {
            Some(value) => unsafe {
                env::set_var(key, value);
            },
            None => unsafe {
                env::remove_var(key);
            },
        }
    }
}
