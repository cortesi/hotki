//! Test bridge protocol used by the smoketest harness to proxy RPCs through the UI.
use std::{
    collections::VecDeque,
    env,
    sync::OnceLock,
    time::{Duration as StdDuration, SystemTime, UNIX_EPOCH},
};

use hotki_protocol::{Cursor, DisplaysSnapshot, NotifyKind};
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
        kind: BridgeKeyKind,
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

/// Key event kind forwarded through the bridge.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BridgeKeyKind {
    /// Simulate a key-down event.
    Down,
    /// Simulate a key-up event.
    Up,
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
        event: BridgeEvent,
    },
    /// Initial handshake response with server/runtime state.
    Handshake {
        /// Current server idle timer snapshot.
        idle_timer: BridgeIdleTimerState,
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
    /// HUD state changed after evaluating a cursor update.
    Hud {
        /// Cursor context describing the HUD state.
        cursor: Cursor,
        /// Logical depth associated with the cursor.
        depth: usize,
        /// Optional parent title when the HUD is nested under another item.
        parent_title: Option<String>,
        /// Keys currently visible in the HUD.
        keys: Vec<BridgeHudKey>,
        /// Display geometry snapshot backing the HUD placement.
        displays: DisplaysSnapshot,
    },
    /// Focus context changed (read-only world stream).
    Focus {
        /// Optional focused app/title/pid context (None when unfocused).
        app: Option<hotki_protocol::App>,
    },
}

/// HUD key metadata forwarded to the smoketest harness.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BridgeHudKey {
    /// Key chord string as rendered by the HUD.
    pub ident: String,
    /// Human-readable description provided by the config.
    pub description: String,
    /// True when the key represents a mode binding.
    pub is_mode: bool,
}

/// Snapshot of the server idle timer state returned during handshake.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BridgeIdleTimerState {
    /// Idle timeout configuration in seconds.
    pub timeout_secs: u64,
    /// True when the timer is currently armed on the server.
    pub armed: bool,
    /// Optional wall-clock deadline for the idle timer in milliseconds since epoch.
    pub deadline_ms: Option<u64>,
    /// Number of clients currently connected to the server.
    pub clients_connected: usize,
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
    let idle_timer = BridgeIdleTimerState {
        timeout_secs: status.idle_timeout_secs,
        armed: status.idle_timer_armed,
        deadline_ms: status.idle_deadline_ms,
        clients_connected: status.clients_connected,
    };
    BridgeResponse::Handshake {
        idle_timer,
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
