//! Test bridge protocol used by the smoketest harness to proxy RPCs through the UI.
use std::{
    collections::VecDeque,
    env,
    io::{self, BufRead, BufReader, Write},
    os::unix::net::UnixStream,
    sync::OnceLock,
    time::{Duration as StdDuration, Instant, SystemTime, UNIX_EPOCH},
};

use hotki_protocol::{DisplaysSnapshot, HudState, NotifyKind, rpc::InjectKind};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tokio::time::{Duration, timeout};
use tracing;

use crate::{Connection, ipc::rpc::ServerStatusLite};

/// Unique identifier assigned to each bridge command.
pub type BridgeCommandId = u64;

/// Millisecond-precision wall-clock timestamp carried by bridge envelopes.
pub type BridgeTimestampMs = u64;

/// Request envelope transmitted from the smoketest harness to the UI runtime.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BridgeCommand {
    /// Monotonic command identifier allocated by the harness.
    pub command_id: BridgeCommandId,
    /// Millisecond timestamp recorded when the harness issued the command.
    pub issued_at_ms: BridgeTimestampMs,
    /// Bridge request payload.
    pub request: BridgeRequest,
}

/// Request type for the smoketest bridge between the smoketest harness and the UI runtime.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum BridgeRequest {
    /// Handshake/ping used to verify the bridge is ready.
    Ping,
    /// Apply a configuration file located at `path`.
    SetConfig {
        /// Filesystem path to the configuration file to load.
        path: String,
    },
    /// Inject a synthetic key event.
    InjectKey {
        /// Identifier to inject (e.g., chord string).
        ident: String,
        /// Key action to perform.
        kind: InjectKind,
        #[serde(default)]
        /// When true, treat the event as a repeat key down.
        repeat: bool,
    },
    /// Fetch the current bindings snapshot.
    GetBindings,
    /// Fetch the current depth for liveness checks.
    GetDepth,
    /// Request a graceful backend shutdown.
    Shutdown,
}

impl BridgeRequest {
    /// Execute a bridge request against a live server connection.
    pub async fn execute(&self, conn: &mut Connection) -> crate::Result<BridgeResponse> {
        match self {
            BridgeRequest::Ping => Err(crate::Error::Ipc(
                "bridge ping must be handled by the UI runtime".to_string(),
            )),
            BridgeRequest::SetConfig { path } => {
                conn.set_config_path(path).await?;
                Ok(BridgeResponse::Ok)
            }
            BridgeRequest::InjectKey {
                ident,
                kind,
                repeat,
            } => {
                match (kind, repeat) {
                    (InjectKind::Down, true) => conn.inject_key_repeat(ident).await?,
                    (InjectKind::Down, false) => conn.inject_key_down(ident).await?,
                    (InjectKind::Up, _) => conn.inject_key_up(ident).await?,
                }
                Ok(BridgeResponse::Ok)
            }
            BridgeRequest::GetBindings => Ok(BridgeResponse::Bindings {
                bindings: conn.get_bindings().await?,
            }),
            BridgeRequest::GetDepth => Ok(BridgeResponse::Depth {
                depth: conn.get_depth().await?,
            }),
            BridgeRequest::Shutdown => {
                conn.shutdown().await?;
                drain_bridge_events(conn, 128, Duration::from_secs(1)).await;
                Ok(BridgeResponse::Ok)
            }
        }
    }
}

/// Response type for the smoketest bridge.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum BridgeResponse {
    /// Acknowledge receipt of a command while it waits in the UI queue.
    Ack {
        /// Number of commands currently queued (including the acknowledged one).
        queued: usize,
    },
    /// Success without additional payload.
    Ok,
    /// Success containing a list of bindings.
    Bindings {
        /// Current bindings snapshot.
        bindings: Vec<String>,
    },
    /// Success containing the current depth.
    Depth {
        /// Current depth value.
        depth: usize,
    },
    /// Asynchronous event emitted by the UI runtime.
    Event {
        /// Event payload describing the observed state change.
        event: Box<BridgeEvent>,
    },
    /// Initial handshake response with server/runtime state.
    Handshake {
        /// Current server idle timer snapshot.
        idle_timer: ServerStatusLite,
        /// Pending notifications queued on the UI side.
        notifications: Vec<BridgeNotification>,
    },
    /// Error with a message for diagnostics.
    Err {
        /// Human-readable error message.
        message: String,
    },
}

/// Event payload streamed from the UI runtime to the smoketest harness.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BridgeEvent {
    /// HUD state changed after evaluating a new render.
    Hud {
        /// Fully rendered HUD state payload.
        hud: Box<HudState>,
        /// Display geometry snapshot backing the HUD placement.
        displays: DisplaysSnapshot,
    },
    /// Focus context changed (read-only world stream).
    Focus {
        /// Optional focused app/title/pid context (None when unfocused).
        app: Option<hotki_protocol::FocusSnapshot>,
    },
}

/// Pending notification payload returned during handshake.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BridgeNotification {
    /// Notification severity kind.
    pub kind: NotifyKind,
    /// Notification title text.
    pub title: String,
    /// Notification body text.
    pub text: String,
}

/// Response envelope emitted by the UI runtime back to the smoketest harness.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BridgeReply {
    /// Identifier of the command that produced this response.
    pub command_id: BridgeCommandId,
    /// Millisecond timestamp recorded when the runtime flushed the response.
    pub timestamp_ms: BridgeTimestampMs,
    /// Response payload.
    pub response: BridgeResponse,
}

/// Errors surfaced by the blocking smoketest bridge transport.
#[derive(Debug, thiserror::Error)]
pub enum BridgeClientError {
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
    /// Bridge IO error while sending/receiving commands.
    #[error("bridge I/O error: {source}")]
    Io {
        /// Underlying IO error.
        #[source]
        source: io::Error,
    },
}

/// Blocking bridge transport that owns socket I/O, command ids, and ACK handling.
pub struct BlockingBridgeClient {
    reader: BufReader<UnixStream>,
    writer: UnixStream,
    socket_path: String,
    next_command_id: BridgeCommandId,
    ack_timeout: StdDuration,
}

impl BlockingBridgeClient {
    /// Connect to the bridge socket using the supplied acknowledgement timeout.
    pub fn connect(path: &str, ack_timeout: StdDuration) -> io::Result<Self> {
        let writer = UnixStream::connect(path)?;
        writer.set_nonblocking(false).ok();
        let reader_stream = writer.try_clone()?;
        Ok(Self {
            reader: BufReader::new(reader_stream),
            writer,
            socket_path: path.to_string(),
            next_command_id: 0,
            ack_timeout,
        })
    }

    /// Return the socket path used by this client.
    pub fn socket_path(&self) -> &str {
        &self.socket_path
    }

    /// Reset command sequencing after a reconnect.
    pub fn reset_command_id(&mut self) {
        self.next_command_id = 0;
    }

    /// Send a bridge request and wait for its final response.
    pub fn call<F>(
        &mut self,
        req: &BridgeRequest,
        mut on_event: F,
    ) -> Result<BridgeResponse, BridgeClientError>
    where
        F: FnMut(BridgeReply),
    {
        let command_id = self.next_command_id;
        let command = BridgeCommand {
            command_id,
            issued_at_ms: now_millis(),
            request: req.clone(),
        };
        self.send_command(&command)?;
        let response = self.await_ack_and_response(command_id, &mut on_event)?;
        self.next_command_id = self.next_command_id.wrapping_add(1);
        Ok(response)
    }

    /// Wait for the next streamed event until `deadline`.
    pub fn wait_for_event_until<F>(
        &mut self,
        deadline: Instant,
        mut on_event: F,
    ) -> Result<bool, BridgeClientError>
    where
        F: FnMut(BridgeReply),
    {
        if Instant::now() >= deadline {
            return Ok(false);
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Ok(false);
        }
        self.reader
            .get_ref()
            .set_read_timeout(Some(remaining))
            .map_err(|source| BridgeClientError::Io { source })?;
        let outcome = self.read_reply();
        if let Err(err) = self.reader.get_ref().set_read_timeout(None) {
            tracing::debug!(?err, "failed to clear bridge read timeout");
        }
        match outcome {
            Ok(reply) => match reply.response {
                BridgeResponse::Event { .. } => {
                    on_event(reply);
                    Ok(true)
                }
                other => Err(BridgeClientError::BridgeFailure {
                    message: format!(
                        "unexpected bridge reply while waiting for events: {:?}",
                        other
                    ),
                }),
            },
            Err(BridgeClientError::Io { source })
                if source.kind() == io::ErrorKind::WouldBlock
                    || source.kind() == io::ErrorKind::TimedOut =>
            {
                Ok(false)
            }
            Err(err) => Err(err),
        }
    }

    fn send_command(&mut self, command: &BridgeCommand) -> Result<(), BridgeClientError> {
        let encoded =
            serde_json::to_string(command).map_err(|err| BridgeClientError::BridgeFailure {
                message: err.to_string(),
            })?;
        self.writer
            .write_all(encoded.as_bytes())
            .map_err(|source| BridgeClientError::Io { source })?;
        self.writer
            .write_all(b"\n")
            .map_err(|source| BridgeClientError::Io { source })?;
        self.writer
            .flush()
            .map_err(|source| BridgeClientError::Io { source })
    }

    fn await_ack_and_response<F>(
        &mut self,
        command_id: BridgeCommandId,
        on_event: &mut F,
    ) -> Result<BridgeResponse, BridgeClientError>
    where
        F: FnMut(BridgeReply),
    {
        self.reader
            .get_ref()
            .set_read_timeout(Some(self.ack_timeout))
            .map_err(|source| BridgeClientError::Io { source })?;
        loop {
            let ack_result = self.read_reply();
            match ack_result {
                Ok(reply) => {
                    if let BridgeResponse::Event { .. } = &reply.response {
                        on_event(reply);
                        continue;
                    }
                    let outcome = self.validate_ack(command_id, &reply);
                    if let Err(err) = self.reader.get_ref().set_read_timeout(None) {
                        tracing::debug!(?err, "failed to clear bridge read timeout");
                    }
                    outcome?;
                    return self.await_final_response(command_id, on_event);
                }
                Err(BridgeClientError::Io { source })
                    if source.kind() == io::ErrorKind::WouldBlock
                        || source.kind() == io::ErrorKind::TimedOut =>
                {
                    if let Err(err) = self.reader.get_ref().set_read_timeout(None) {
                        tracing::debug!(?err, "failed to clear bridge read timeout");
                    }
                    return Err(BridgeClientError::AckTimeout {
                        command_id,
                        timeout_ms: self.ack_timeout.as_millis() as u64,
                    });
                }
                Err(err) => {
                    if let Err(clear_err) = self.reader.get_ref().set_read_timeout(None) {
                        tracing::debug!(?clear_err, "failed to clear bridge read timeout");
                    }
                    return Err(err);
                }
            }
        }
    }

    fn validate_ack(
        &self,
        command_id: BridgeCommandId,
        ack: &BridgeReply,
    ) -> Result<(), BridgeClientError> {
        if ack.command_id != command_id {
            return Err(BridgeClientError::SequenceMismatch {
                expected: command_id,
                got: ack.command_id,
            });
        }
        match &ack.response {
            BridgeResponse::Ack { queued } => {
                tracing::debug!(command_id, queued, "bridge_ack");
                Ok(())
            }
            BridgeResponse::Err { message } => Err(BridgeClientError::BridgeFailure {
                message: message.clone(),
            }),
            _ => Err(BridgeClientError::AckMissing { command_id }),
        }
    }

    fn await_final_response<F>(
        &mut self,
        command_id: BridgeCommandId,
        on_event: &mut F,
    ) -> Result<BridgeResponse, BridgeClientError>
    where
        F: FnMut(BridgeReply),
    {
        loop {
            let reply = self.read_reply()?;
            if let BridgeResponse::Event { .. } = &reply.response {
                on_event(reply);
                continue;
            }
            if reply.command_id != command_id {
                return Err(BridgeClientError::SequenceMismatch {
                    expected: command_id,
                    got: reply.command_id,
                });
            }
            return match reply.response {
                BridgeResponse::Ack { .. } => Err(BridgeClientError::AckMissing { command_id }),
                BridgeResponse::Err { message } => Ok(BridgeResponse::Err { message }),
                other => Ok(other),
            };
        }
    }

    fn read_reply(&mut self) -> Result<BridgeReply, BridgeClientError> {
        let mut line = String::new();
        let bytes = self
            .reader
            .read_line(&mut line)
            .map_err(|source| BridgeClientError::Io { source })?;
        if bytes == 0 {
            return Err(BridgeClientError::BridgeFailure {
                message: format!("bridge socket '{}' closed", self.socket_path),
            });
        }
        let trimmed = line.trim_end_matches(['\n', '\r']);
        serde_json::from_str(trimmed).map_err(|err| BridgeClientError::BridgeFailure {
            message: err.to_string(),
        })
    }
}

impl BridgeResponse {
    /// Map the response into a `Result`, discarding the payload.
    pub fn into_result(self) -> Result<(), String> {
        match self {
            BridgeResponse::Ok => Ok(()),
            BridgeResponse::Err { message } => Err(message),
            other => Err(format!("unexpected bridge response: {:?}", other)),
        }
    }

    /// Extract a payload of bindings from the response.
    pub fn into_bindings(self) -> Result<Vec<String>, String> {
        match self {
            BridgeResponse::Bindings { bindings } => Ok(bindings),
            BridgeResponse::Err { message } => Err(message),
            other => Err(format!("unexpected bridge response: {:?}", other)),
        }
    }

    /// Extract a depth value from the response.
    pub fn into_depth(self) -> Result<usize, String> {
        match self {
            BridgeResponse::Depth { depth } => Ok(depth),
            BridgeResponse::Err { message } => Err(message),
            other => Err(format!("unexpected bridge response: {:?}", other)),
        }
    }
}

/// Buffer for pending notifications carried in bridge handshakes.
#[derive(Default, Clone)]
pub struct BridgeNotifications {
    max: usize,
    buf: VecDeque<BridgeNotification>,
}

impl BridgeNotifications {
    /// Create a buffer with a maximum capacity.
    pub fn new(max: usize) -> Self {
        Self {
            max,
            buf: VecDeque::new(),
        }
    }

    /// Record a notification, evicting the oldest when capacity is reached.
    pub fn record(&mut self, kind: NotifyKind, title: &str, text: &str) {
        if self.buf.len() >= self.max {
            self.buf.pop_front();
        }
        self.buf.push_back(BridgeNotification {
            kind,
            title: title.to_string(),
            text: text.to_string(),
        });
    }

    /// Clear tracked notifications.
    pub fn clear(&mut self) {
        self.buf.clear();
    }

    /// Snapshot notifications for handshake payloads.
    pub fn snapshot(&self) -> Vec<BridgeNotification> {
        self.buf.iter().cloned().collect()
    }
}

/// Build a handshake response from a server status snapshot and pending notifications.
pub fn handshake_response(
    status: &ServerStatusLite,
    notifications: Vec<BridgeNotification>,
) -> BridgeResponse {
    BridgeResponse::Handshake {
        idle_timer: status.clone(),
        notifications,
    }
}

/// Drain pending bridge events after shutdown to avoid post-stop chatter.
pub async fn drain_bridge_events(
    conn: &mut Connection,
    max_events: usize,
    per_event_timeout: Duration,
) {
    let mut processed = 0usize;
    while processed < max_events {
        match timeout(per_event_timeout, conn.recv_event()).await {
            Ok(Ok(_)) => {
                processed += 1;
            }
            Ok(Err(crate::Error::Ipc(ref s))) if s == "Event channel closed" => {
                break;
            }
            Ok(Err(err)) => {
                tracing::debug!(?err, "bridge drain aborted");
                break;
            }
            Err(_) => break,
        }
    }
    if processed >= max_events {
        tracing::debug!("bridge drain reached event limit");
    }
}

/// Override slot for control socket path selection.
static CONTROL_SOCKET_OVERRIDE: OnceLock<Mutex<Option<String>>> = OnceLock::new();

fn control_socket_override_slot() -> &'static Mutex<Option<String>> {
    CONTROL_SOCKET_OVERRIDE.get_or_init(|| Mutex::new(None))
}

/// Guard that scopes a custom control socket path for the bridge driver.
pub struct ControlSocketScope {
    previous: Option<String>,
}

impl ControlSocketScope {
    /// Install a new override, restoring the prior path on drop.
    pub fn new(path: impl Into<String>) -> Self {
        let mut slot = control_socket_override_slot().lock();
        let previous = slot.replace(path.into());
        Self { previous }
    }
}

impl Drop for ControlSocketScope {
    fn drop(&mut self) {
        let mut slot = control_socket_override_slot().lock();
        *slot = self.previous.take();
    }
}

/// Derive the control socket path from the server socket path.
pub fn control_socket_path(server_socket: &str) -> String {
    if let Some(path) = control_socket_override_slot().lock().clone() {
        return path;
    }
    if let Some(path) = env::var_os("HOTKI_CONTROL_SOCKET")
        && let Some(value) = path.to_str()
    {
        return value.to_string();
    }
    format!("{server_socket}.bridge")
}

/// Return the current wall-clock timestamp in milliseconds since the Unix epoch.
pub fn now_millis() -> BridgeTimestampMs {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_else(|_| StdDuration::from_secs(0))
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}
