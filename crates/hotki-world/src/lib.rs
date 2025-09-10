//! hotki-world: Window State Service (skeleton)
//!
//! Maintains types and constructor for the World service.
//! This stage provides the public API surface only; implementation arrives in later stages.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use mac_winops::ops::WinOps;
use mac_winops::{Pos, WindowId};
use tokio::sync::{broadcast, mpsc, oneshot};

/// Unique key for a window.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct WindowKey {
    pub pid: i32,
    pub id: WindowId,
}

/// Opaque metadata attached to a window.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct WindowMeta;

/// Identifier for a display.
pub type DisplayId = u32;

/// Snapshot of a single window.
#[derive(Clone, Debug)]
pub struct WorldWindow {
    pub app: String,
    pub title: String,
    pub pid: i32,
    pub id: WindowId,
    pub pos: Option<Pos>,
    pub layer: i32,
    pub z: u32,
    pub on_active_space: bool,
    pub display_id: Option<DisplayId>,
    pub focused: bool,
    pub meta: Vec<WindowMeta>,
    pub last_seen: Instant,
    pub seen_seq: u64,
}

/// Permission state for capabilities that affect data quality.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PermissionState {
    Granted,
    Denied,
    Unknown,
}

impl Default for PermissionState {
    fn default() -> Self {
        Self::Unknown
    }
}

/// Reported capabilities and current permission state.
#[derive(Clone, Debug, Default)]
pub struct Capabilities {
    pub accessibility: PermissionState,
    pub screen_recording: PermissionState,
}

/// Configuration for the world service.
#[derive(Clone, Debug)]
pub struct WorldCfg {
    pub poll_ms_min: u64,
    pub poll_ms_max: u64,
    pub include_offscreen: bool,
    pub ax_watch_frontmost: bool,
}

impl Default for WorldCfg {
    fn default() -> Self {
        Self {
            poll_ms_min: 100,
            poll_ms_max: 1000,
            include_offscreen: false,
            ax_watch_frontmost: false,
        }
    }
}

/// Delta describing changed fields (placeholder for Stage 1).
#[derive(Clone, Debug, Default)]
pub struct WindowDelta;

/// World events stream payloads.
#[derive(Clone, Debug)]
pub enum WorldEvent {
    Added(WorldWindow),
    Removed(WindowKey),
    Updated(WindowKey, WindowDelta),
    MetaAdded(WindowKey, WindowMeta),
    MetaRemoved(WindowKey, WindowMeta),
    FocusChanged(Option<WindowKey>),
}

/// Cheap, clonable handle to the world service.
#[derive(Clone, Debug)]
pub struct WorldHandle {
    tx: mpsc::UnboundedSender<Command>,
    events: broadcast::Sender<WorldEvent>,
}

impl WorldHandle {
    /// Subscribe to the global event stream (no filters in Stage 2).
    pub fn subscribe(&self) -> broadcast::Receiver<WorldEvent> {
        self.events.subscribe()
    }

    /// Get a full snapshot of current windows.
    pub async fn snapshot(&self) -> Vec<WorldWindow> {
        let (tx, rx) = oneshot::channel();
        let _ = self.tx.send(Command::Snapshot { respond: tx });
        rx.await.unwrap_or_default()
    }

    /// Lookup a window by key.
    pub async fn get(&self, key: WindowKey) -> Option<WorldWindow> {
        let (tx, rx) = oneshot::channel();
        let _ = self.tx.send(Command::Get { key, respond: tx });
        rx.await.unwrap_or(None)
    }

    /// Current focused window key, if any.
    pub async fn focused(&self) -> Option<WindowKey> {
        let (tx, rx) = oneshot::channel();
        let _ = self.tx.send(Command::Focused { respond: tx });
        rx.await.unwrap_or(None)
    }

    /// Current capabilities and permission state.
    pub async fn capabilities(&self) -> Capabilities {
        let (tx, rx) = oneshot::channel();
        let _ = self.tx.send(Command::Capabilities { respond: tx });
        rx.await.unwrap_or_default()
    }
}

/// World constructor. Spawns the service and returns a handle.
///
/// Stage 1: define the API surface. Implementation is added in later stages.
pub struct World;

impl World {
    #[allow(unused_variables)]
    pub fn spawn(winops: Arc<dyn WinOps>, cfg: WorldCfg) -> WorldHandle {
        let (tx, rx) = mpsc::unbounded_channel();
        // Keep event buffer moderate; callers should keep up.
        let (evt_tx, _evt_rx) = broadcast::channel(256);

        let state = WorldState::new();
        tokio::spawn(run_actor(rx, evt_tx.clone(), state));

        WorldHandle { tx, events: evt_tx }
    }
}

// ===== Stage 2: Actor + Storage =====

#[derive(Clone, Debug, Default)]
struct WorldState {
    store: HashMap<WindowKey, WorldWindow>,
    focused: Option<WindowKey>,
    capabilities: Capabilities,
    seen_seq: u64,
}

impl WorldState {
    fn new() -> Self {
        Self {
            store: HashMap::new(),
            focused: None,
            capabilities: Capabilities::default(),
            seen_seq: 0,
        }
    }
}

enum Command {
    Snapshot {
        respond: oneshot::Sender<Vec<WorldWindow>>,
    },
    Get {
        key: WindowKey,
        respond: oneshot::Sender<Option<WorldWindow>>,
    },
    Focused {
        respond: oneshot::Sender<Option<WindowKey>>,
    },
    Capabilities {
        respond: oneshot::Sender<Capabilities>,
    },
}

async fn run_actor(
    mut rx: mpsc::UnboundedReceiver<Command>,
    _events: broadcast::Sender<WorldEvent>,
    state: WorldState,
) {
    while let Some(cmd) = rx.recv().await {
        match cmd {
            Command::Snapshot { respond } => {
                let mut v: Vec<WorldWindow> = state.store.values().cloned().collect();
                v.sort_by_key(|w| (w.z, w.pid, w.id));
                let _ = respond.send(v);
            }
            Command::Get { key, respond } => {
                let _ = respond.send(state.store.get(&key).cloned());
            }
            Command::Focused { respond } => {
                let _ = respond.send(state.focused);
            }
            Command::Capabilities { respond } => {
                let _ = respond.send(state.capabilities.clone());
            }
        }
    }
}
