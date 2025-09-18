//! hotki-world: Window State Service
//!
//! Single source of truth for window and focus state on macOS.
//!
//! Focus rules:
//! - Prefer Accessibility (AX) focus when available, fall back to CoreGraphics
//!   heuristics. When AX identifies the focused window, its AX title is
//!   preferred over the CG title for better fidelity.
//!
//! Event debounce:
//! - `Updated` events are coalesced with a ~50ms debounce to reduce chatter
//!   during rapid title/geometry changes. Snapshots always reflect latest state.
//!
//! Display mapping:
//! - Each window is mapped to the display with the greatest overlap of its
//!   bounds among current screens.
//!
//! Trait boundary:
//! - Downstream crates consume the [`WorldView`] trait for snapshots, focus
//!   context, and refresh hints. Convenience methods such as
//!   [`WorldView::frontmost_window`], [`WorldView::window_by_pid_title`], and
//!   [`WorldView::resolve_key`] replace the old `view_util::*` helpers.
//!
//! # Stable API Surface
//! The following items form the supported interface for other crates:
//! - [`World`] and [`WorldHandle`] for constructing and querying the service.
//! - [`WorldView`] for trait-object access to snapshots, focus, and helpers such
//!   as [`WorldView::frontmost_window`] and [`WorldView::window_by_pid_title`].
//! - Data carriers [`WorldWindow`], [`WorldEvent`], [`WorldStatus`],
//!   [`Capabilities`], [`PermissionState`], and [`WindowKey`] describing the
//!   snapshot contract.
//!
//! # Test Utilities
//! Enable the `test-utils` feature to pull in [`test_support`] and [`test_api`]
//! for integration tests. These remain available unconditionally inside this
//! crate's own test suite.
#![warn(missing_docs)]
#![warn(unsafe_op_in_unsafe_fn)]

use std::{
    collections::HashMap,
    sync::{Arc, OnceLock},
    time::{Duration, Instant},
};

pub use hotki_world_ids::WorldWindowId;
use mac_winops::{AxProps, Pos, WindowId, WindowInfo, ops::WinOps};
use tokio::{
    sync::{broadcast, mpsc, oneshot},
    time::{Instant as TokioInstant, sleep},
};

// Test-only visibility: stash a clone of the AX bridge sender for unit tests.
static AX_BRIDGE_SENDER: OnceLock<
    parking_lot::Mutex<Option<crossbeam_channel::Sender<mac_winops::AxEvent>>>,
> = OnceLock::new();

/// Unique key for a window.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct WindowKey {
    /// Process identifier that owns the window.
    pub pid: i32,
    /// Core Graphics window id (`kCGWindowNumber`). Stable for the lifetime of the window.
    pub id: WindowId,
}

/// Opaque metadata attached to a window.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct WindowMeta;

/// Identifier for a display.
type DisplayId = u32;

/// Display bounds tuple: `(display_id, x, y, width, height)` in Cocoa screen coordinates.
type DisplayBounds = (DisplayId, i32, i32, i32, i32);

/// Snapshot of a single window.
#[derive(Clone, Debug)]
pub struct WorldWindow {
    /// Human-readable application name (from CoreGraphics `kCGWindowOwnerName`).
    pub app: String,
    /// Window title.
    ///
    /// If Accessibility is granted and the window is focused, the title prefers the
    /// AX value for the focused window for better fidelity; otherwise the CoreGraphics
    /// title is used. When Screen Recording permission is denied, some titles may be
    /// blank as macOS may redact them.
    pub title: String,
    /// Owning process id.
    pub pid: i32,
    /// Core Graphics window id (`kCGWindowNumber`).
    pub id: WindowId,
    /// Window bounds in screen coordinates, when known.
    /// `None` if the window is off-screen or CoreGraphics did not report bounds.
    pub pos: Option<Pos>,
    /// CoreGraphics window layer (0 = standard app windows). Non-zero layers are
    /// overlays such as HUD/notification windows.
    pub layer: i32,
    /// Monotonic z-order index within the current snapshot: 0 is frontmost, larger
    /// values are farther back. Derived from CoreGraphics enumeration order.
    pub z: u32,
    /// Mission Control space identifier reported by CoreGraphics.
    pub space: Option<mac_winops::SpaceId>,
    /// True if the window is observed on the active Space in the current snapshot.
    pub on_active_space: bool,
    /// True if CoreGraphics reports the window as currently on-screen.
    pub is_on_screen: bool,
    /// Identifier of the display with the largest overlap of the window's bounds, if any.
    pub display_id: Option<u32>,
    /// True if this is the focused window according to AX (preferred) or CG fallback.
    pub focused: bool,
    /// AX properties/capabilities for the focused window (None for non‑focused).
    pub ax: Option<AxProps>,
    /// Opaque metadata tags associated with the window (reserved for future use).
    pub meta: Vec<WindowMeta>,
    /// Timestamp when this window was last observed during reconciliation.
    pub last_seen: Instant,
    /// Sequence number of the last scan in which this window was seen. Useful for
    /// debugging and tests.
    pub seen_seq: u64,
}

impl WorldWindow {
    /// Identifier pairing the owning process id with the window id.
    #[must_use]
    pub fn world_id(&self) -> WorldWindowId {
        WorldWindowId::new(self.pid, self.id)
    }
}

impl From<&WorldWindow> for WorldWindowId {
    fn from(value: &WorldWindow) -> Self {
        value.world_id()
    }
}

/// Permission state for capabilities that affect data quality.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PermissionState {
    /// Permission is granted.
    Granted,
    /// Permission is explicitly denied.
    Denied,
    /// Permission has not been determined yet.
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
    /// Accessibility permission. Needed for accurate focus tracking and for
    /// reading AX titles of the focused window.
    pub accessibility: PermissionState,
    /// Screen Recording permission. Without this, CoreGraphics may redact some
    /// window titles, resulting in blank titles.
    pub screen_recording: PermissionState,
}

/// Configuration for the world service.
#[derive(Clone, Debug)]
pub struct WorldCfg {
    /// Minimum polling interval in milliseconds. Used immediately after changes or
    /// on a refresh hint.
    pub poll_ms_min: u64,
    /// Maximum polling interval in milliseconds when idle. The actor exponentially
    /// backs off up to this bound.
    pub poll_ms_max: u64,
    /// Reserved for future use. Off‑screen windows are currently not enumerated.
    pub include_offscreen: bool,
    /// Reserved for future use. AX frontmost watching is not yet enabled here.
    pub ax_watch_frontmost: bool,
    /// Broadcast buffer size for world events. If consumers lag behind,
    /// older events are dropped and receivers observe `RecvError::Lagged`.
    pub events_buffer: usize,
}

impl Default for WorldCfg {
    fn default() -> Self {
        Self {
            poll_ms_min: 100,
            poll_ms_max: 1000,
            include_offscreen: false,
            ax_watch_frontmost: false,
            events_buffer: 256,
        }
    }
}

/// Delta describing changed fields (placeholder for Stage 1).
#[derive(Clone, Debug, Default)]
pub struct WindowDelta;

/// World events stream payloads.
#[derive(Clone, Debug)]
pub enum WorldEvent {
    /// A new window was observed. Carries the initial snapshot of that window.
    Added(Box<WorldWindow>),
    /// A previously observed window disappeared from the active Space.
    Removed(WindowKey),
    /// A window's properties changed. Updates are coalesced with a ~50ms debounce
    /// to avoid flooding on rapid changes.
    Updated(WindowKey, WindowDelta),
    /// A metadata tag was attached to a window (reserved for future use).
    MetaAdded(WindowKey, WindowMeta),
    /// A metadata tag was removed from a window (reserved for future use).
    MetaRemoved(WindowKey, WindowMeta),
    /// The focused window changed. `None` indicates no focused window.
    FocusChanged(Option<WindowKey>),
}

/// Diagnostic snapshot of world internals.
#[derive(Clone, Debug, Default)]
pub struct WorldStatus {
    /// Number of windows currently tracked.
    pub windows_count: usize,
    /// Key of the currently focused window, if any.
    pub focused: Option<WindowKey>,
    /// Time spent (ms) in the most recent reconciliation pass.
    pub last_tick_ms: u64,
    /// Current polling interval (ms) after backoff/adaptation.
    pub current_poll_ms: u64,
    /// Size of the internal debounce cache used to coalesce updates.
    pub debounce_cache: usize,
    /// Reported capability/permission state affecting data quality.
    pub capabilities: Capabilities,
}

/// Cheap, clonable handle to the world service.
#[derive(Clone, Debug)]
pub struct WorldHandle {
    tx: mpsc::UnboundedSender<Command>,
    events: broadcast::Sender<WorldEvent>,
}

impl WorldHandle {
    /// Subscribe to the world event stream.
    ///
    /// The stream includes Added/Updated/Removed and FocusChanged events. Callers
    /// should drain events promptly to avoid backpressure on the broadcast buffer.
    pub fn subscribe(&self) -> broadcast::Receiver<WorldEvent> {
        self.events.subscribe()
    }

    /// Subscribe and fetch a consistent snapshot + focused key from the actor.
    ///
    /// The snapshot and focused key are produced atomically relative to each
    /// other. Events may already be buffered in the returned receiver; treat
    /// the snapshot as baseline and then apply subsequent events.
    pub async fn subscribe_with_snapshot(
        &self,
    ) -> (
        broadcast::Receiver<WorldEvent>,
        Vec<WorldWindow>,
        Option<WindowKey>,
    ) {
        let rx = self.events.subscribe();
        let (tx, rx_once) = oneshot::channel();
        let _ = self.tx.send(Command::SnapshotFocus { respond: tx });
        let (snap, focused) = rx_once.await.unwrap_or_default();
        (rx, snap, focused)
    }

    /// Get a full snapshot of current windows.
    ///
    /// The returned vector is not sorted; callers may sort by `z` or other fields
    /// if a specific order is required.
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

    /// Current focused window with full info, if any.
    pub async fn focused_window(&self) -> Option<WorldWindow> {
        let (tx, rx) = oneshot::channel();
        let _ = self.tx.send(Command::FocusedWindow { respond: tx });
        rx.await.unwrap_or(None)
    }

    /// Convenience accessor for `(app, title, pid)` of the focused window.
    pub async fn focused_context(&self) -> Option<(String, String, i32)> {
        self.focused_window().await.map(|w| (w.app, w.title, w.pid))
    }

    /// Subscribe to world events and return an initial focus context, if any.
    ///
    /// The seed context is derived atomically relative to the returned
    /// snapshot+focused pair, but exposed here as a concise tuple to simplify
    /// downstream consumers.
    pub async fn subscribe_with_context(
        &self,
    ) -> (
        broadcast::Receiver<WorldEvent>,
        Option<(String, String, i32)>,
    ) {
        let (rx, snap, focused) = self.subscribe_with_snapshot().await;
        let ctx = if let Some(fk) = focused {
            snap.iter()
                .find(|w| w.pid == fk.pid && w.id == fk.id)
                .map(|w| (w.app.clone(), w.title.clone(), w.pid))
        } else {
            snap.iter()
                .min_by_key(|w| w.z)
                .map(|w| (w.app.clone(), w.title.clone(), w.pid))
        };
        (rx, ctx)
    }

    /// Resolve a key to a lightweight context tuple `(app, title, pid)`.
    pub async fn context_for_key(&self, key: WindowKey) -> Option<(String, String, i32)> {
        self.get(key).await.map(|w| (w.app, w.title, w.pid))
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

    // AxProps are exposed via `WorldWindow.ax` on snapshots/focused_window.
}

/// World constructor. Spawns the background actor and returns a handle.
///
/// The actor continuously reconciles CoreGraphics/AX window state on a dynamic
/// polling interval between `poll_ms_min` and `poll_ms_max`, emitting events to
/// subscribers and serving snapshots via the handle.
pub struct World;

mod ax_read_pool;
mod view;

#[cfg(any(test, feature = "test-utils"))]
pub use view::TestWorld;
pub use view::WorldView;

impl World {
    #[allow(unused_variables)]
    /// Start the world service.
    ///
    /// - `winops`: abstraction for window enumeration and helpers (real or mock)
    /// - `cfg`: tuning parameters for polling/backoff
    ///
    /// Returns a [`WorldHandle`] for querying snapshots and subscribing to events.
    pub fn spawn(winops: Arc<dyn WinOps>, cfg: WorldCfg) -> WorldHandle {
        let (tx, rx) = mpsc::unbounded_channel();
        // Keep event buffer moderate; callers should keep up.
        let (evt_tx, _evt_rx) = broadcast::channel(cfg.events_buffer.max(8));

        let state = WorldState::new();
        tokio::spawn(run_actor(rx, evt_tx.clone(), state, winops, cfg.clone()));

        let handle = WorldHandle { tx, events: evt_tx };

        // Initialize the per‑PID AX read pool and give it a handle to nudge
        // the world actor when reads complete.
        ax_read_pool::init(handle.tx.clone());

        // Bridge macOS AX observer events into world refresh hints with light
        // throttling to coalesce bursts (e.g., AXTitleChanged storms).
        // Throttle window: 16ms; send immediately if idle longer than that.
        let (tx_ax, rx_ax) = crossbeam_channel::unbounded::<mac_winops::AxEvent>();
        // Expose for tests (cloned)
        AX_BRIDGE_SENDER
            .get_or_init(|| parking_lot::Mutex::new(None))
            .lock()
            .replace(tx_ax.clone());
        mac_winops::set_ax_observer_sender(tx_ax);
        let hint_handle = handle.clone();
        std::thread::Builder::new()
            .name("ax-hint-bridge".to_string())
            .spawn(move || {
                let mut last = Instant::now() - Duration::from_millis(32);
                let min_gap = Duration::from_millis(16);
                while let Ok(_ev) = rx_ax.recv() {
                    let now = Instant::now();
                    let since = now.saturating_duration_since(last);
                    if since >= min_gap {
                        hint_handle.hint_refresh();
                        last = now;
                    } else {
                        let wait = min_gap - since;
                        std::thread::sleep(wait);
                        hint_handle.hint_refresh();
                        last = Instant::now();
                        // Drain any burst quickly; coalesce to a single hint
                        while rx_ax.try_recv().is_ok() {}
                    }
                }
            })
            .ok();

        handle
    }

    /// Spawn the world service and return it as a trait object.
    pub fn spawn_view(winops: Arc<dyn WinOps>, cfg: WorldCfg) -> Arc<dyn WorldView> {
        Arc::new(Self::spawn(winops, cfg))
    }

    /// Spawn a no-op world suitable for tests. Responds immediately with
    /// default/empty data and emits no events. No polling or background work.
    pub fn spawn_noop() -> WorldHandle {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let (evt_tx, _evt_rx) = broadcast::channel(8);
        tokio::spawn(async move {
            while let Some(cmd) = rx.recv().await {
                match cmd {
                    Command::Snapshot { respond } => {
                        let _ = respond.send(Vec::new());
                    }
                    Command::Get { respond, .. } => {
                        let _ = respond.send(None);
                    }
                    Command::Focused { respond } => {
                        let _ = respond.send(None);
                    }
                    Command::FocusedWindow { respond } => {
                        let _ = respond.send(None);
                    }
                    Command::SnapshotFocus { respond } => {
                        let _ = respond.send((Vec::new(), None));
                    }
                    Command::Capabilities { respond } => {
                        let _ = respond.send(Capabilities::default());
                    }
                    Command::HintRefresh => {}
                    Command::Status { respond } => {
                        let _ = respond.send(WorldStatus::default());
                    }
                }
            }
        });
        WorldHandle { tx, events: evt_tx }
    }

    /// Spawn the no-op world as a trait object for dependency injection in tests.
    pub fn spawn_noop_view() -> Arc<dyn WorldView> {
        Arc::new(Self::spawn_noop())
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
    /// Pending coalesced Updated events with their flush deadline.
    coalesce: HashMap<WindowKey, Instant>,
    last_tick_ms: u64,
    current_poll_ms: u64,
    warned_ax: bool,
    warned_screen: bool,
    /// Windows that have gone missing recently and are pending confirmation
    /// before eviction. Value is the number of consecutive misses observed.
    suspects: HashMap<WindowKey, u8>,
}

impl WorldState {
    fn new() -> Self {
        Self {
            store: HashMap::new(),
            focused: None,
            capabilities: Capabilities::default(),
            seen_seq: 0,
            last_emit: HashMap::new(),
            coalesce: HashMap::new(),
            last_tick_ms: 0,
            current_poll_ms: 0,
            warned_ax: false,
            warned_screen: false,
            suspects: HashMap::new(),
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
    FocusedWindow {
        respond: oneshot::Sender<Option<WorldWindow>>,
    },
    /// Snapshot and focused key together for consistent seeding.
    SnapshotFocus {
        respond: oneshot::Sender<(Vec<WorldWindow>, Option<WindowKey>)>,
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
    // Immediate first reconcile (no initial delay)
    next_tick.as_mut().reset(TokioInstant::now());

    // Coalesce timer: sleeps until earliest pending coalesce deadline.
    let mut next_coalesce_due: Option<Instant>;
    let coalesce_tick = sleep(Duration::from_millis(1_000));
    tokio::pin!(coalesce_tick);
    // Start disabled
    coalesce_tick.as_mut().reset(TokioInstant::from_std(
        Instant::now() + Duration::from_secs(3600),
    ));

    // Perform an immediate first reconcile to seed state before serving requests
    update_capabilities(&mut state);
    let t0 = Instant::now();
    let _ = reconcile(&mut state, &events, &*winops);
    state.last_tick_ms = t0.elapsed().as_millis() as u64;
    state.current_poll_ms = current_ms;

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
                    Command::FocusedWindow { respond } => {
                        let w = state.focused.and_then(|k| state.store.get(&k).cloned());
                        let _ = respond.send(w);
                    }
                    Command::SnapshotFocus { respond } => {
                        let mut v: Vec<WorldWindow> = state.store.values().cloned().collect();
                        v.sort_by_key(|w| (w.z, w.pid, w.id));
                        let _ = respond.send((v, state.focused));
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

                // Update coalesce timer to earliest pending deadline
                next_coalesce_due = state.coalesce.values().copied().min();
                if let Some(due) = next_coalesce_due {
                    coalesce_tick.as_mut().reset(TokioInstant::from_std(due));
                }
            }
            _ = &mut coalesce_tick => {
                let now = Instant::now();
                // Emit coalesced updates whose deadline has passed
                let keys: Vec<WindowKey> = state
                    .coalesce
                    .iter()
                    .filter_map(|(k, &due)| if due <= now { Some(*k) } else { None })
                    .collect();
                for k in keys.iter() {
                    if state.store.contains_key(k) {
                        let _ = events.send(WorldEvent::Updated(*k, WindowDelta));
                        state.last_emit.insert(*k, now);
                    }
                    state.coalesce.remove(k);
                }
                // Re-arm the timer for the next earliest deadline
                next_coalesce_due = state.coalesce.values().copied().min();
                if let Some(due) = next_coalesce_due {
                    coalesce_tick.as_mut().reset(TokioInstant::from_std(due));
                } else {
                    // No pending deadlines; park far in the future
                    coalesce_tick.as_mut().reset(TokioInstant::from_std(Instant::now() + Duration::from_secs(3600)));
                }
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

    let wins: Vec<WindowInfo> = winops.list_windows_for_spaces(&[]);
    let mut had_changes = false;

    // Focus: prefer AX/system snapshot when available; fall back to CG-derived focus flag.
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

    let front_pid = wins
        .iter()
        .find(|w| w.layer == 0)
        .map(|w| w.pid)
        .or_else(|| wins.first().map(|w| w.pid));

    if let Some(pid) = front_pid
        && acc_ok()
        && let Some(ax_id) = ax_read_pool::focused_id(pid)
    {
        let candidate = WindowKey { pid, id: ax_id };
        if wins.iter().any(|w| w.pid == pid && w.id == ax_id) {
            ax_focus_title = ax_read_pool::title(pid, ax_id);
            ax_focus_key = Some(candidate);
            tracing::debug!(
                pid,
                ax_id,
                title = ax_focus_title.as_deref().unwrap_or(""),
                "reconcile: accepted ax focus candidate"
            );
        }
    }

    let new_focused = ax_focus_key.or(cg_focus_key);

    // Build key set and additions/updates
    let mut seen_keys: Vec<WindowKey> = Vec::with_capacity(wins.len());

    // Cache display bounds for this reconcile pass.
    let displays = list_display_bounds();

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
            if existing.space != w.space {
                existing.space = w.space;
                changed = true;
            }
            if existing.on_active_space != w.on_active_space {
                existing.on_active_space = w.on_active_space;
                changed = true;
            }
            if existing.is_on_screen != w.is_on_screen {
                existing.is_on_screen = w.is_on_screen;
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
            // Populate AX props only for the focused window; clear otherwise.
            existing.ax = if is_focus {
                ax_read_pool::props(w.pid, w.id)
            } else {
                None
            };
            existing.last_seen = now;
            existing.seen_seq = seq;
            if changed {
                had_changes = true;
                // Trailing-edge debounce: schedule coalesced Updated after quiet period
                state.coalesce.insert(key, now + Duration::from_millis(50));
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
                space: w.space,
                on_active_space: w.on_active_space,
                is_on_screen: w.is_on_screen,
                display_id,
                focused: is_focus,
                ax: if is_focus {
                    ax_read_pool::props(w.pid, w.id)
                } else {
                    None
                },
                meta: Vec::new(),
                last_seen: now,
                seen_seq: seq,
            };
            state.store.insert(key, ww.clone());
            let _ = events.send(WorldEvent::Added(Box::new(ww)));
            state.last_emit.insert(key, now);
        }
    }

    // Removals with suspect confirmation
    const SUSPECT_MISSES: u8 = 1; // mark suspect after 1 missed pass; evict on next if still absent
    let seen: std::collections::HashSet<_> = seen_keys.iter().copied().collect();
    let existing_keys: Vec<_> = state.store.keys().copied().collect();
    // First, clear suspect status for any windows we have seen this pass.
    for key in seen.iter() {
        state.suspects.remove(key);
    }
    for key in existing_keys {
        if !seen.contains(&key) {
            let misses = state.suspects.entry(key).or_insert(0);
            *misses = misses.saturating_add(1);
            if *misses > SUSPECT_MISSES {
                // Confirm absence against a fresh CGWindowList filtered by the same pid/id
                let still_absent = {
                    let confirm = winops.list_windows_for_spaces(&[]);
                    !confirm.iter().any(|w| w.pid == key.pid && w.id == key.id)
                };
                if still_absent {
                    had_changes = true;
                    state.store.remove(&key);
                    state.last_emit.remove(&key);
                    state.coalesce.remove(&key);
                    state.suspects.remove(&key);
                    let _ = events.send(WorldEvent::Removed(key));
                }
            }
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

fn best_display_id(pos: &Pos, displays: &[DisplayBounds]) -> Option<DisplayId> {
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

// ===== AX and permission shims (overridable for tests) =====
use parking_lot::Mutex;
thread_local! { static TEST_OVERRIDES: Mutex<TestOverrides> = Mutex::new(TestOverrides::default()); }

#[derive(Default, Clone)]
struct TestOverrides {
    acc_ok: Option<bool>,
    ax_focus: Option<(i32, u32)>,
    ax_title: Option<(u32, String)>,
    displays: Option<Vec<DisplayBounds>>,
    ax_delay_title_ms: Option<u64>,
    ax_delay_focus_ms: Option<u64>,
    ax_async_only: Option<bool>,
}

fn acc_ok() -> bool {
    if let Some(v) = TEST_OVERRIDES.with(|o| o.lock().acc_ok) {
        return v;
    }
    permissions::accessibility_ok()
}

fn ax_focused_window_id_for_pid(pid: i32) -> Option<u32> {
    // Optional test delay
    if let Some(ms) = TEST_OVERRIDES.with(|o| o.lock().ax_delay_focus_ms) {
        std::thread::sleep(std::time::Duration::from_millis(ms));
    }
    if let Some((p, id)) = TEST_OVERRIDES.with(|o| o.lock().ax_focus)
        && p == pid
    {
        return Some(id);
    }
    mac_winops::ax_focused_window_id_for_pid(pid)
}

fn ax_title_for_window_id(id: u32) -> Option<String> {
    // Optional test delay
    if let Some(ms) = TEST_OVERRIDES.with(|o| o.lock().ax_delay_title_ms) {
        std::thread::sleep(std::time::Duration::from_millis(ms));
    }
    if let Some((tid, title)) = TEST_OVERRIDES.with(|o| o.lock().ax_title.clone())
        && tid == id
    {
        return Some(title);
    }
    mac_winops::ax_title_for_window_id(id)
}

fn list_display_bounds() -> Vec<DisplayBounds> {
    if let Some(v) = TEST_OVERRIDES.with(|o| o.lock().displays.clone()) {
        return v;
    }
    mac_winops::screen::list_display_bounds()
}

#[doc(hidden)]
pub mod test_api {
    use std::time::Duration;

    use tokio::time::{Instant, sleep};

    use super::{DisplayBounds, TEST_OVERRIDES, TestOverrides, WorldWindow};

    /// Get a clone of the AX hint bridge sender (if initialized) for tests.
    pub fn ax_hint_bridge_sender() -> Option<crossbeam_channel::Sender<mac_winops::AxEvent>> {
        super::AX_BRIDGE_SENDER
            .get()
            .and_then(|m| m.lock().as_ref().cloned())
    }
    pub fn set_accessibility_ok(v: bool) {
        TEST_OVERRIDES.with(|o| {
            let mut s = o.lock();
            s.acc_ok = Some(v);
        });
    }
    pub fn set_ax_focus(pid: i32, id: u32) {
        TEST_OVERRIDES.with(|o| {
            let mut s = o.lock();
            s.ax_focus = Some((pid, id));
        });
    }
    // set_ax_title now forwards to the AX read pool's cross-thread override so worker threads
    // observe the value. For legacy behavior (thread-local override), prefer using the private
    // helper inside unit tests.
    pub fn set_displays(v: Vec<DisplayBounds>) {
        TEST_OVERRIDES.with(|o| {
            let mut s = o.lock();
            s.displays = Some(v);
        });
    }
    pub fn clear() {
        TEST_OVERRIDES.with(|o| {
            let mut s = o.lock();
            *s = TestOverrides::default();
        });
    }

    // ===== AX read pool test helpers =====
    pub fn ensure_ax_pool_inited() {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        super::ax_read_pool::init(tx);
    }
    pub fn ax_pool_reset_metrics_and_cache() {
        super::ax_read_pool::_test_reset_metrics_and_cache();
    }
    pub fn ax_pool_metrics() -> (usize, usize) {
        super::ax_read_pool::_test_inflight_metrics()
    }
    pub fn ax_pool_stale_drop_count() -> usize {
        super::ax_read_pool::_test_stale_drop_count()
    }
    pub fn ax_pool_peek_title(pid: i32, id: u32) -> Option<String> {
        super::ax_read_pool::_test_peek_title(pid, id)
    }
    pub fn ax_pool_schedule_title(pid: i32, id: u32) -> Option<String> {
        super::ax_read_pool::title(pid, id)
    }
    pub fn ax_pool_schedule_focus(pid: i32) -> Option<u32> {
        super::ax_read_pool::focused_id(pid)
    }
    pub fn set_ax_delay_title_ms(ms: u64) {
        // Prefer pool-level override so worker threads see it.
        super::ax_read_pool::_test_set_title_delay_ms(ms);
    }
    pub fn set_ax_delay_focus_ms(ms: u64) {
        TEST_OVERRIDES.with(|o| {
            let mut s = o.lock();
            s.ax_delay_focus_ms = Some(ms);
        });
    }
    pub fn set_ax_async_only(v: bool) {
        TEST_OVERRIDES.with(|o| {
            let mut s = o.lock();
            s.ax_async_only = Some(v);
        });
    }
    pub fn set_ax_title(id: u32, title: &str) {
        // Set both: thread-local (for synchronous override path) and
        // pool-level (so worker threads also observe it).
        let t = title.to_string();
        TEST_OVERRIDES.with(|o| {
            let mut s = o.lock();
            s.ax_title = Some((id, t));
        });
        super::ax_read_pool::_test_set_title_override(id, title);
    }

    /// Await until the world snapshot satisfies `pred`, up to `timeout_ms`.
    /// Returns true if the predicate matched in time.
    use crate::WorldView;

    pub async fn wait_snapshot_until<F, W>(world: &W, timeout_ms: u64, mut pred: F) -> bool
    where
        W: WorldView + ?Sized,
        F: FnMut(&[WorldWindow]) -> bool,
    {
        let deadline = Instant::now() + Duration::from_millis(timeout_ms);
        loop {
            let snap = world.snapshot().await;
            if pred(&snap) {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            sleep(Duration::from_millis(2)).await;
        }
    }
}

/// Test support utilities exported for the test suite.
#[doc(hidden)]
pub mod test_support;
