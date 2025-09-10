//! hotki-world: Window State Service (skeleton)
//!
//! Maintains types and constructor for the World service.
//! This stage provides the public API surface only; implementation arrives in later stages.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use mac_winops::ops::WinOps;
use mac_winops::{Pos, WindowId, WindowInfo};
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio::time::{Instant as TokioInstant, sleep};

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
        tokio::spawn(run_actor(rx, evt_tx.clone(), state, winops, cfg));

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
    last_emit: HashMap<WindowKey, Instant>,
}

impl WorldState {
    fn new() -> Self {
        Self {
            store: HashMap::new(),
            focused: None,
            capabilities: Capabilities::default(),
            seen_seq: 0,
            last_emit: HashMap::new(),
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
    /// Hint that focus/frontmost likely changed: trigger immediate reconcile.
    HintRefresh,
}

async fn run_actor(
    mut rx: mpsc::UnboundedReceiver<Command>,
    events: broadcast::Sender<WorldEvent>,
    mut state: WorldState,
    winops: Arc<dyn WinOps>,
    cfg: WorldCfg,
) {
    let mut current_ms = cfg.poll_ms_min.max(10);
    let next_tick = sleep(Duration::from_millis(current_ms));
    tokio::pin!(next_tick);

    loop {
        tokio::select! {
            maybe_cmd = rx.recv() => {
                let Some(cmd) = maybe_cmd else { break };
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
                    Command::HintRefresh => {
                        current_ms = cfg.poll_ms_min;
                        next_tick.as_mut().reset(TokioInstant::now());
                    }
                }
            }
            _ = &mut next_tick => {
                let had_changes = reconcile(&mut state, &events, &*winops);
                if had_changes { current_ms = cfg.poll_ms_min; }
                else { current_ms = (current_ms + 50).min(cfg.poll_ms_max.max(current_ms)); }
                next_tick.as_mut().reset(TokioInstant::now() + Duration::from_millis(current_ms));
            }
        }
    }
}

fn reconcile(
    state: &mut WorldState,
    events: &broadcast::Sender<WorldEvent>,
    winops: &dyn WinOps,
) -> bool {
    let now = Instant::now();
    state.seen_seq = state.seen_seq.wrapping_add(1);
    let seq = state.seen_seq;

    let wins: Vec<WindowInfo> = winops.list_windows();
    let mut had_changes = false;

    // Build key set and additions/updates
    let mut seen_keys: Vec<WindowKey> = Vec::with_capacity(wins.len());
    let mut new_focused: Option<WindowKey> = None;

    for (idx, w) in wins.iter().enumerate() {
        let key = WindowKey {
            pid: w.pid,
            id: w.id,
        };
        seen_keys.push(key);
        if w.focused {
            new_focused = Some(key);
        }
        let z = idx as u32;
        if let Some(existing) = state.store.get_mut(&key) {
            let mut changed = false;
            if existing.title != w.title {
                existing.title = w.title.clone();
                changed = true;
            }
            if existing.layer != w.layer {
                existing.layer = w.layer;
                changed = true;
            }
            if existing.pos != w.pos {
                existing.pos = w.pos;
                changed = true;
            }
            if existing.z != z {
                existing.z = z;
                changed = true;
            }
            if !existing.on_active_space {
                existing.on_active_space = true;
                changed = true;
            }
            existing.last_seen = now;
            existing.seen_seq = seq;
            if changed {
                had_changes = true;
                let do_emit = match state.last_emit.get(&key) {
                    Some(t) => now.duration_since(*t) >= Duration::from_millis(50),
                    None => true,
                };
                if do_emit {
                    let _ = events.send(WorldEvent::Updated(key, WindowDelta));
                    state.last_emit.insert(key, now);
                }
            }
        } else {
            had_changes = true;
            let ww = WorldWindow {
                app: w.app.clone(),
                title: w.title.clone(),
                pid: w.pid,
                id: w.id,
                pos: w.pos,
                layer: w.layer,
                z,
                on_active_space: true,
                display_id: None,
                focused: w.focused,
                meta: Vec::new(),
                last_seen: now,
                seen_seq: seq,
            };
            state.store.insert(key, ww.clone());
            let _ = events.send(WorldEvent::Added(ww));
            state.last_emit.insert(key, now);
        }
    }

    // Removals
    let seen: std::collections::HashSet<_> = seen_keys.iter().copied().collect();
    let existing_keys: Vec<_> = state.store.keys().copied().collect();
    for key in existing_keys {
        if !seen.contains(&key) {
            had_changes = true;
            state.store.remove(&key);
            state.last_emit.remove(&key);
            let _ = events.send(WorldEvent::Removed(key));
        }
    }

    // Focus changes
    if state.focused != new_focused {
        state.focused = new_focused;
        let _ = events.send(WorldEvent::FocusChanged(new_focused));
    }

    had_changes
}

impl WorldHandle {
    /// Hint that the frontmost app/window likely changed; triggers immediate refresh.
    pub fn hint_refresh(&self) {
        let _ = self.tx.send(Command::HintRefresh);
    }
}
