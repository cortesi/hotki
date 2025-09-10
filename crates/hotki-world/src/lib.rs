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

/// Diagnostic snapshot of world internals.
#[derive(Clone, Debug, Default)]
pub struct WorldStatus {
    pub windows_count: usize,
    pub focused: Option<WindowKey>,
    pub last_tick_ms: u64,
    pub current_poll_ms: u64,
    pub debounce_cache: usize,
    pub capabilities: Capabilities,
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

    /// Get internal diagnostics: counts, timings, permissions.
    pub async fn status(&self) -> WorldStatus {
        let (tx, rx) = oneshot::channel();
        let _ = self.tx.send(Command::Status { respond: tx });
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
    last_tick_ms: u64,
    current_poll_ms: u64,
    warned_ax: bool,
    warned_screen: bool,
}

impl WorldState {
    fn new() -> Self {
        Self {
            store: HashMap::new(),
            focused: None,
            capabilities: Capabilities::default(),
            seen_seq: 0,
            last_emit: HashMap::new(),
            last_tick_ms: 0,
            current_poll_ms: 0,
            warned_ax: false,
            warned_screen: false,
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
    /// Return diagnostics.
    Status {
        respond: oneshot::Sender<WorldStatus>,
    },
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
                    Command::Status { respond } => {
                        let status = WorldStatus {
                            windows_count: state.store.len(),
                            focused: state.focused,
                            last_tick_ms: state.last_tick_ms,
                            current_poll_ms: state.current_poll_ms,
                            debounce_cache: state.last_emit.len(),
                            capabilities: state.capabilities.clone(),
                        };
                        let _ = respond.send(status);
                    }
                }
            }
            _ = &mut next_tick => {
                // Update permissions; warn once if missing.
                update_capabilities(&mut state);
                let t0 = Instant::now();
                let had_changes = reconcile(&mut state, &events, &*winops);
                state.last_tick_ms = t0.elapsed().as_millis() as u64;
                if had_changes { current_ms = cfg.poll_ms_min; }
                else { current_ms = (current_ms + 50).min(cfg.poll_ms_max.max(current_ms)); }
                state.current_poll_ms = current_ms;
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

    // Focus: prefer AX if available; fall back to CG-derived focus flag.
    let cg_focus_key = wins
        .iter()
        .find(|w| w.layer == 0 && w.focused)
        .map(|w| WindowKey {
            pid: w.pid,
            id: w.id,
        })
        .or_else(|| {
            wins.first().map(|w| WindowKey {
                pid: w.pid,
                id: w.id,
            })
        });

    let mut ax_focus_key: Option<WindowKey> = None;
    let mut ax_focus_title: Option<String> = None;
    if permissions::accessibility_ok()
        && let Some(front_pid) = wins
            .iter()
            .find(|w| w.layer == 0)
            .map(|w| w.pid)
            .or_else(|| wins.first().map(|w| w.pid))
        && let Some(ax_id) = mac_winops::ax_focused_window_id_for_pid(front_pid)
    {
        let candidate = WindowKey {
            pid: front_pid,
            id: ax_id,
        };
        if wins
            .iter()
            .any(|w| w.pid == candidate.pid && w.id == candidate.id)
        {
            ax_focus_title = mac_winops::ax_title_for_window_id(ax_id);
            ax_focus_key = Some(candidate);
        }
    }

    let new_focused = ax_focus_key.or(cg_focus_key);

    // Build key set and additions/updates
    let mut seen_keys: Vec<WindowKey> = Vec::with_capacity(wins.len());

    // Cache display bounds for this reconcile pass.
    let displays = mac_winops::screen::list_display_bounds();

    for (idx, w) in wins.iter().enumerate() {
        let key = WindowKey {
            pid: w.pid,
            id: w.id,
        };
        seen_keys.push(key);
        let is_focus = Some(key) == new_focused;
        let z = idx as u32;
        let display_id = match (w.pos, displays.is_empty()) {
            (Some(pos), false) => best_display_id(&pos, &displays),
            _ => None,
        };
        if let Some(existing) = state.store.get_mut(&key) {
            let mut changed = false;
            let new_title = if is_focus {
                ax_focus_title.clone().unwrap_or_else(|| w.title.clone())
            } else {
                w.title.clone()
            };
            if existing.title != new_title {
                existing.title = new_title;
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
            if existing.display_id != display_id {
                existing.display_id = display_id;
                changed = true;
            }
            if existing.focused != is_focus {
                existing.focused = is_focus;
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
                title: if is_focus {
                    ax_focus_title.clone().unwrap_or_else(|| w.title.clone())
                } else {
                    w.title.clone()
                },
                pid: w.pid,
                id: w.id,
                pos: w.pos,
                layer: w.layer,
                z,
                on_active_space: true,
                display_id,
                focused: is_focus,
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

fn update_capabilities(state: &mut WorldState) {
    let ax_ok = permissions::accessibility_ok();
    let sr_ok = permissions::screen_recording_ok();
    state.capabilities.accessibility = if ax_ok {
        PermissionState::Granted
    } else {
        PermissionState::Denied
    };
    state.capabilities.screen_recording = if sr_ok {
        PermissionState::Granted
    } else {
        PermissionState::Denied
    };
    if !ax_ok && !state.warned_ax {
        tracing::warn!(
            "Accessibility permission denied: focus/title quality will degrade. Grant access in System Settings > Privacy & Security > Accessibility."
        );
        state.warned_ax = true;
    }
    if !sr_ok && !state.warned_screen {
        tracing::warn!(
            "Screen Recording permission denied: some window titles may be blank. Grant access in System Settings > Privacy & Security > Screen Recording."
        );
        state.warned_screen = true;
    }
}

fn best_display_id(pos: &Pos, displays: &[(u32, i32, i32, i32, i32)]) -> Option<u32> {
    let (x, y, w, h) = (pos.x, pos.y, pos.width, pos.height);
    let l1 = x;
    let t1 = y;
    let r1 = x + w;
    let b1 = y + h;
    let mut best_id = None;
    let mut best_area: i64 = 0;
    for &(id, dx, dy, dw, dh) in displays.iter() {
        let l2 = dx;
        let t2 = dy;
        let r2 = dx + dw;
        let b2 = dy + dh;
        let iw = (r1.min(r2) - l1.max(l2)).max(0) as i64;
        let ih = (b1.min(b2) - t1.max(t2)).max(0) as i64;
        let area = iw * ih;
        if area > best_area {
            best_area = area;
            best_id = Some(id);
        }
    }
    if best_area > 0 { best_id } else { None }
}
