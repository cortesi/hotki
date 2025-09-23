#![allow(clippy::disallowed_methods)]
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
//!   during rapid title/geometry changes. Tests can override the debounce window
//!   to accelerate timing-sensitive scenarios. Snapshots always reflect latest
//!   state.
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
    collections::{HashMap, HashSet},
    fmt,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

pub use hotki_world_ids::WorldWindowId;
use mac_winops::{
    self, AxProps, Error as WinOpsError, PlaceAttemptOptions as WinPlaceAttemptOptions, Pos,
    WindowId, WindowInfo, ops::WinOps, screen,
};
use regex::Regex;

use self::events::{DEFAULT_EVENT_CAPACITY, EventHub};

/// Re-export of placement attempt tuning options used by mac-winops.
pub type PlaceAttemptOptions = WinPlaceAttemptOptions;

/// Strategy used to bring a window forward during placement operations.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RaiseStrategy {
    /// Do not attempt to raise the window before placement.
    None,
    /// Activate the owning application before placement attempts.
    AppActivate,
    /// Keep the currently frontmost window ahead of the target during placement.
    KeepFrontWindow,
    /// Time-boxed raise that prefers AX/CG ordering over app activation.
    SmartRaise {
        /// Overall deadline allowed for the raise attempt.
        deadline: Duration,
    },
}

/// Policy applied when the target window is minimized during placement.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MinimizedPolicy {
    /// Defer placement until the window is restored by the caller.
    DeferUntilUnminimized,
    /// Automatically unminimize the window before placement.
    AutoUnminimize,
}

/// High-level placement tuning options shared across helpers and mimics.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PlaceOptions {
    /// Strategy for raising the window prior to placement.
    pub raise: RaiseStrategy,
    /// Policy applied when the window is minimized.
    pub minimized: MinimizedPolicy,
    /// Whether to request animated placement behaviour when supported.
    pub animate: bool,
}

impl Default for PlaceOptions {
    fn default() -> Self {
        Self {
            raise: RaiseStrategy::AppActivate,
            minimized: MinimizedPolicy::AutoUnminimize,
            animate: true,
        }
    }
}
use tokio::{
    sync::{mpsc, oneshot},
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

/// Identifier for a command issued through the world orchestration layer.
pub type CommandId = u64;

/// High-level command categories handled by the world service.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommandKind {
    /// Place a window onto an absolute grid cell.
    PlaceGrid,
    /// Move a window relative to its current grid cell.
    PlaceMoveGrid,
    /// Toggle hide/unhide behaviour for an application's windows.
    Hide,
    /// Enter or exit fullscreen.
    Fullscreen,
    /// Raise a specific window to the foreground.
    Raise,
    /// Shift focus in a direction.
    FocusDir,
}

/// Describes how the target window was chosen for a command.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TargetSelection {
    /// The currently focused window satisfied the request.
    Focused,
    /// The active-space frontmost window was chosen.
    ActiveFrontmost,
    /// Selection cycled among matched candidates.
    Cycle,
    /// Explicit world window id was requested.
    Explicit,
}

impl TargetSelection {
    fn as_str(self) -> &'static str {
        match self {
            Self::Focused => "focused",
            Self::ActiveFrontmost => "active-frontmost",
            Self::Cycle => "cycle",
            Self::Explicit => "explicit",
        }
    }
}

/// Toggle semantics for world-managed commands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommandToggle {
    /// Explicitly enable the behaviour.
    On,
    /// Explicitly disable the behaviour.
    Off,
    /// Toggle between on/off.
    Toggle,
}

/// Fullscreen flavour requested by a client.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FullscreenKind {
    /// Native macOS fullscreen transition.
    Native,
    /// Nonnative fullscreen handled by window APIs.
    Nonnative,
}

/// Direction for move-based placement commands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MoveDirection {
    /// Move left on the grid.
    Left,
    /// Move right on the grid.
    Right,
    /// Move up on the grid.
    Up,
    /// Move down on the grid.
    Down,
}

/// Intent for an absolute placement request.
#[derive(Clone, Debug)]
pub struct PlaceIntent {
    /// Number of columns in the target grid.
    pub cols: u32,
    /// Number of rows in the target grid.
    pub rows: u32,
    /// Column index to place the window at.
    pub col: u32,
    /// Row index to place the window at.
    pub row: u32,
    /// Optional preferred process identifier for selection.
    pub pid_hint: Option<i32>,
    /// Explicit world window target, when provided.
    pub target: Option<WorldWindowId>,
    /// Optional placement tuning overrides.
    pub options: Option<PlaceAttemptOptions>,
}

/// Intent for a relative placement request.
#[derive(Clone, Debug)]
pub struct MoveIntent {
    /// Number of columns in the target grid.
    pub cols: u32,
    /// Number of rows in the target grid.
    pub rows: u32,
    /// Direction to move within the grid.
    pub dir: MoveDirection,
    /// Optional preferred process identifier for selection.
    pub pid_hint: Option<i32>,
    /// Explicit world window target, when provided.
    pub target: Option<WorldWindowId>,
    /// Optional placement tuning overrides.
    pub options: Option<PlaceAttemptOptions>,
}

/// Intent for hide/show commands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HideIntent {
    /// Desired toggle behaviour.
    pub desired: CommandToggle,
}

/// Intent for fullscreen commands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FullscreenIntent {
    /// Desired toggle behaviour.
    pub desired: CommandToggle,
    /// Fullscreen flavour.
    pub kind: FullscreenKind,
}

/// Intent for raise commands using optional regex filters.
#[derive(Clone, Debug)]
pub struct RaiseIntent {
    /// Optional application name regex.
    pub app_regex: Option<Arc<Regex>>,
    /// Optional window title regex.
    pub title_regex: Option<Arc<Regex>>,
}

impl RaiseIntent {
    /// Return true when a world window matches the configured filters.
    fn matches(&self, window: &WorldWindow) -> bool {
        let app_ok = self
            .app_regex
            .as_ref()
            .map(|regex| regex.is_match(&window.app))
            .unwrap_or(true);
        let title_ok = self
            .title_regex
            .as_ref()
            .map(|regex| regex.is_match(&window.title))
            .unwrap_or(true);
        app_ok && title_ok
    }
}

/// Outcome of a world command accepted by the orchestration layer.
#[derive(Clone, Debug)]
pub struct CommandReceipt {
    /// Unique identifier assigned to this command.
    pub id: CommandId,
    /// Command category.
    pub kind: CommandKind,
    /// Issue timestamp (monotonic clock).
    pub issued_at: Instant,
    /// Target window chosen for the command, if any.
    pub target: Option<WorldWindow>,
    /// Selection strategy explanation.
    pub selection: Option<TargetSelection>,
}

impl CommandReceipt {
    /// Helper returning the world window id if a target was selected.
    #[must_use]
    pub fn target_id(&self) -> Option<WorldWindowId> {
        self.target.as_ref().map(WorldWindow::world_id)
    }
}

/// Errors returned by world-orchestrated command handling.
#[derive(Debug)]
pub enum CommandError {
    /// No window matched the request on the active space.
    NoEligibleWindow {
        /// Command category.
        kind: CommandKind,
        /// Optional process identifier hint.
        pid: Option<i32>,
    },
    /// The matching window resides on a different Mission Control space.
    OffActiveSpace {
        /// Process identifier involved in the request.
        pid: i32,
        /// Space identifier, when known.
        space: Option<mac_winops::SpaceId>,
    },
    /// Backend failure while invoking macOS window operations.
    BackendFailure {
        /// Command category.
        kind: CommandKind,
        /// Error message from the backend.
        message: String,
    },
    /// Request was invalid (e.g., missing context).
    InvalidRequest {
        /// Description of the invalid condition.
        message: String,
    },
}

impl CommandError {
    fn backend(kind: CommandKind, err: impl fmt::Display) -> Self {
        Self::BackendFailure {
            kind,
            message: err.to_string(),
        }
    }
}

impl fmt::Display for CommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CommandError::NoEligibleWindow { kind, pid } => {
                write!(f, "No eligible window for {:?}", kind)?;
                if let Some(pid) = pid {
                    write!(f, " (pid={})", pid)?;
                }
                Ok(())
            }
            CommandError::OffActiveSpace { pid: _, space } => match space {
                Some(space_id) => write!(
                    f,
                    "Window is on Mission Control space {}. Switch back to that space and try again.",
                    space_id
                ),
                None => write!(f, "Window is not on the active Mission Control space."),
            },
            CommandError::BackendFailure { message, .. } => f.write_str(message),
            CommandError::InvalidRequest { message } => f.write_str(message),
        }
    }
}

impl std::error::Error for CommandError {}

/// Permission state for capabilities that affect data quality.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PermissionState {
    /// Permission is granted.
    Granted,
    /// Permission is explicitly denied.
    Denied,
    /// Permission has not been determined yet.
    #[default]
    Unknown,
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
            events_buffer: DEFAULT_EVENT_CAPACITY,
        }
    }
}

/// Captures the previous and current value for a window field that changed.
#[derive(Clone, Debug)]
pub struct ValueChange<T> {
    /// Value observed before the change was applied.
    pub old: T,
    /// Value observed after the change was applied.
    pub new: T,
}

impl<T> ValueChange<T> {
    #[must_use]
    fn new(old: T, new: T) -> Self {
        Self { old, new }
    }
}

/// Field-level delta attached to [`WorldEvent::Updated`].
#[derive(Clone, Debug, Default)]
pub struct WindowDelta {
    /// Title change.
    pub title: Option<ValueChange<String>>,
    /// Layer change.
    pub layer: Option<ValueChange<i32>>,
    /// Geometry change.
    pub pos: Option<ValueChange<Option<Pos>>>,
    /// Z-order change.
    pub z: Option<ValueChange<u32>>,
    /// Space assignment change.
    pub space: Option<ValueChange<Option<mac_winops::SpaceId>>>,
    /// Active-space residency change.
    pub on_active_space: Option<ValueChange<bool>>,
    /// On-screen residency change.
    pub is_on_screen: Option<ValueChange<bool>>,
    /// Display identifier change.
    pub display_id: Option<ValueChange<Option<DisplayId>>>,
    /// Focus flag change.
    pub focused: Option<ValueChange<bool>>,
}

impl WindowDelta {
    fn is_empty(&self) -> bool {
        self.title.is_none()
            && self.layer.is_none()
            && self.pos.is_none()
            && self.z.is_none()
            && self.space.is_none()
            && self.on_active_space.is_none()
            && self.is_on_screen.is_none()
            && self.display_id.is_none()
            && self.focused.is_none()
    }

    fn merge(&mut self, other: WindowDelta) {
        if let Some(change) = other.title {
            match &mut self.title {
                Some(existing) => existing.new = change.new,
                None => self.title = Some(change),
            }
        }
        if let Some(change) = other.layer {
            match &mut self.layer {
                Some(existing) => existing.new = change.new,
                None => self.layer = Some(change),
            }
        }
        if let Some(change) = other.pos {
            match &mut self.pos {
                Some(existing) => existing.new = change.new,
                None => self.pos = Some(change),
            }
        }
        if let Some(change) = other.z {
            match &mut self.z {
                Some(existing) => existing.new = change.new,
                None => self.z = Some(change),
            }
        }
        if let Some(change) = other.space {
            match &mut self.space {
                Some(existing) => existing.new = change.new,
                None => self.space = Some(change),
            }
        }
        if let Some(change) = other.on_active_space {
            match &mut self.on_active_space {
                Some(existing) => existing.new = change.new,
                None => self.on_active_space = Some(change),
            }
        }
        if let Some(change) = other.is_on_screen {
            match &mut self.is_on_screen {
                Some(existing) => existing.new = change.new,
                None => self.is_on_screen = Some(change),
            }
        }
        if let Some(change) = other.display_id {
            match &mut self.display_id {
                Some(existing) => existing.new = change.new,
                None => self.display_id = Some(change),
            }
        }
        if let Some(change) = other.focused {
            match &mut self.focused {
                Some(existing) => existing.new = change.new,
                None => self.focused = Some(change),
            }
        }
    }
}

/// Context describing the current focus selection accompanying focus events.
#[derive(Clone, Debug, Default)]
pub struct FocusChange {
    /// Window key for the focused window, when available.
    pub key: Option<WindowKey>,
    /// Focused window's application name.
    pub app: Option<String>,
    /// Focused window's title.
    pub title: Option<String>,
    /// Focused window's process identifier.
    pub pid: Option<i32>,
}

/// World events stream payloads.
#[derive(Clone, Debug)]
pub enum WorldEvent {
    /// A new window was observed. Carries the initial snapshot of that window.
    Added(Box<WorldWindow>),
    /// A previously observed window disappeared from the active Space.
    Removed(WindowKey),
    /// A window's properties changed. Updates are coalesced with a ~50ms debounce
    /// to avoid flooding on rapid changes (tests may override the debounce interval).
    /// Each event carries field-level diffs for the changed properties.
    Updated(WindowKey, WindowDelta),
    /// A metadata tag was attached to a window (reserved for future use).
    MetaAdded(WindowKey, WindowMeta),
    /// A metadata tag was removed from a window (reserved for future use).
    MetaRemoved(WindowKey, WindowMeta),
    /// The focused window changed, including best-effort context.
    FocusChanged(FocusChange),
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
    /// Number of pending coalesced updates waiting for their quiet-period deadline.
    pub debounce_pending: usize,
    /// Monotonic reconciliation sequence number (increments once per reconcile pass).
    pub reconcile_seq: u64,
    /// Pending suspect entries awaiting confirmation before eviction.
    pub suspects_pending: usize,
    /// Reported capability/permission state affecting data quality.
    pub capabilities: Capabilities,
}

/// Lightweight snapshot of internal world metrics used for test instrumentation.
#[derive(Clone, Debug, Default)]
pub struct WorldMetricsSnapshot {
    /// Number of windows currently tracked.
    pub windows_count: usize,
    /// Pending coalesced update count.
    pub debounce_pending: usize,
    /// Pending suspect removals awaiting confirmation.
    pub suspects_pending: usize,
    /// Latest reconciliation sequence number.
    pub reconcile_seq: u64,
}

/// Diagnostic counts used to verify world teardown quiescence.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct QuiescenceReport {
    /// Active Accessibility observers managed by mac-winops.
    pub active_ax_observers: usize,
    /// Pending main-thread operations queued in mac-winops.
    pub pending_main_ops: usize,
    /// Mimic windows currently alive (tests/dev harness only).
    pub mimic_windows: usize,
    /// Active event stream subscriptions on the world handle.
    pub subscriptions: usize,
}

impl QuiescenceReport {
    /// True when all diagnostic counters are zero.
    #[must_use]
    pub fn is_quiescent(&self) -> bool {
        self.active_ax_observers == 0
            && self.pending_main_ops == 0
            && self.mimic_windows == 0
            && self.subscriptions == 0
    }
}

/// Cheap, clonable handle to the world service.
#[derive(Clone)]
pub struct WorldHandle {
    tx: mpsc::UnboundedSender<Command>,
    events: Arc<EventHub>,
    metrics: Arc<WorldMetrics>,
}

impl WorldHandle {
    /// Subscribe to the world event stream.
    ///
    /// The stream includes Added/Updated/Removed and FocusChanged events. Callers
    /// should drain events promptly to avoid overrunning the per-subscription ring buffer
    /// (capacity configured via [`WorldCfg::events_buffer`], default 16,384 entries).
    pub fn subscribe(&self) -> EventCursor {
        self.events.subscribe(None)
    }

    /// Subscribe to the world event stream using the provided filter.
    pub fn subscribe_with_filter(&self, filter: EventFilter) -> EventCursor {
        self.events.subscribe(Some(filter))
    }

    /// Subscribe to the world event stream using a closure-based filter.
    pub fn subscribe_filtered<F>(&self, filter: F) -> EventCursor
    where
        F: Fn(&WorldEvent) -> bool + Send + Sync + 'static,
    {
        self.subscribe_with_filter(Arc::new(filter))
    }

    /// Await the next event for a cursor until the deadline is reached.
    pub async fn next_event_until(
        &self,
        cursor: &mut EventCursor,
        deadline: TokioInstant,
    ) -> Option<WorldEvent> {
        self.events.next_event_until(cursor, deadline).await
    }

    /// Attempt to take the next event without waiting.
    pub fn next_event_now(&self, cursor: &mut EventCursor) -> Option<WorldEvent> {
        self.events.try_next(cursor)
    }

    /// Snapshot lightweight internal metrics without asking the actor.
    pub fn metrics_snapshot(&self) -> WorldMetricsSnapshot {
        self.metrics.snapshot()
    }

    /// Collect counts used to verify teardown quiescence.
    #[must_use]
    pub fn quiescence_report(&self) -> QuiescenceReport {
        QuiescenceReport {
            active_ax_observers: mac_winops::active_ax_observer_count(),
            pending_main_ops: mac_winops::pending_main_ops_len(),
            mimic_windows: mac_winops::mimic_window_count(),
            subscriptions: self.events.subscriber_count(),
        }
    }

    /// True when the world reports no lingering observers, ops, or subscriptions.
    #[must_use]
    pub fn is_quiescent(&self) -> bool {
        self.quiescence_report().is_quiescent()
    }

    /// Reset helper state by closing subscriptions, mimic windows, and draining main ops.
    pub fn reset(&self) -> QuiescenceReport {
        self.events.close_all();
        mac_winops::close_mimic_windows();
        while mac_winops::pending_main_ops() {
            mac_winops::drain_main_ops();
        }
        crate::ax_read_pool::reset();
        let _ = mac_winops::clear_ax_observers();
        if mac_winops::active_ax_observer_count() > 0 {
            let deadline = Instant::now() + Duration::from_millis(200);
            while mac_winops::active_ax_observer_count() > 0 && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(5));
            }
        }
        self.quiescence_report()
    }

    /// Snapshot recent events published by the world, newest last.
    pub fn recent_events(&self, limit: usize) -> Vec<EventRecord> {
        self.events.recent_events(limit)
    }

    /// Subscribe and fetch a consistent snapshot + focused key from the actor.
    ///
    /// The snapshot and focused key are produced atomically relative to each
    /// other. Events may already be buffered in the returned receiver; treat
    /// the snapshot as baseline and then apply subsequent events.
    pub async fn subscribe_with_snapshot(
        &self,
    ) -> (EventCursor, Vec<WorldWindow>, Option<WindowKey>) {
        let cursor = self.subscribe();
        let (tx, rx_once) = oneshot::channel();
        let _ = self.tx.send(Command::SnapshotFocus { respond: tx });
        let (snap, focused) = rx_once.await.unwrap_or_default();
        (cursor, snap, focused)
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

    /// Snapshot of current frame metadata for all tracked windows.
    pub async fn frames_snapshot(&self) -> HashMap<WindowKey, Frames> {
        let (tx, rx) = oneshot::channel();
        let _ = self.tx.send(Command::FramesSnapshot { respond: tx });
        rx.await.unwrap_or_default()
    }

    /// Retrieve frame metadata for a specific window key.
    pub async fn frames(&self, key: WindowKey) -> Option<Frames> {
        self.frames_snapshot().await.get(&key).cloned()
    }

    /// Resolve the display scale associated with a display identifier, if tracked.
    pub async fn display_scale(&self, display_id: u32) -> Option<f32> {
        self.frames_snapshot()
            .await
            .values()
            .find(|frames| frames.display_id == Some(display_id))
            .map(|frames| frames.scale)
    }

    /// Compute the default epsilon for a display based on tracked frame scale information.
    pub async fn authoritative_eps(&self, display_id: u32) -> i32 {
        let frames = self.frames_snapshot().await;
        let scale = frames
            .values()
            .find(|f| f.display_id == Some(display_id))
            .map(|f| f.scale)
            .unwrap_or(1.0);
        default_eps(scale)
    }

    /// Pump pending main-thread operations until `deadline` or the queue empties.
    ///
    /// Returns `true` when all operations completed before the deadline, `false`
    /// otherwise.
    pub fn pump_main_until(&self, deadline: Instant) -> bool {
        loop {
            mac_winops::drain_main_ops();
            if !mac_winops::pending_main_ops() {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            if let Err(err) = mac_winops::focus::post_user_event() {
                tracing::trace!(?err, "pump_main_until: post_user_event failed");
            }
            if Instant::now() >= deadline {
                tracing::trace!("pump_main_until: deadline reached before wait");
                return false;
            }
            if !mac_winops::wait_main_ops_idle(deadline) {
                tracing::trace!(
                    pending = mac_winops::pending_main_ops_len(),
                    "pump_main_until: wait_main_ops_idle timed out"
                );
                return false;
            }
        }
    }

    /// Construct an observer that waits for events on `key` using the default configuration.
    #[must_use]
    pub fn window_observer(&self, key: WindowKey) -> WindowObserver {
        self.window_observer_with_config(key, WaitConfig::default())
    }

    /// Construct an observer that waits for events on `key` using the supplied configuration.
    #[must_use]
    pub fn window_observer_with_config(
        &self,
        key: WindowKey,
        config: WaitConfig,
    ) -> WindowObserver {
        wait::make_window_observer(self, key, config)
    }

    /// Await the appearance of a window satisfying `predicate` with the default wait configuration.
    pub async fn await_window_where<F>(
        &self,
        condition: &'static str,
        predicate: F,
    ) -> Result<WorldWindow, WaitError>
    where
        F: Fn(&WorldWindow) -> bool + Send + Sync + 'static,
    {
        self.await_window_where_with_config(condition, predicate, WaitConfig::default())
            .await
    }

    /// Await the appearance of a window satisfying `predicate` using the supplied configuration.
    pub async fn await_window_where_with_config<F>(
        &self,
        condition: &'static str,
        predicate: F,
        config: WaitConfig,
    ) -> Result<WorldWindow, WaitError>
    where
        F: Fn(&WorldWindow) -> bool + Send + Sync + 'static,
    {
        wait::await_window_matching(self, Arc::new(predicate), condition, config).await
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

    /// Queue a grid placement command.
    ///
    /// The request resolves the target window inside `hotki-world`, invokes the
    /// macOS window operation, and returns once the command is scheduled. The
    /// returned [`CommandReceipt`] includes the chosen target, if any. Listen to
    /// [`WorldEvent::Updated`] events for the target's [`WorldWindowId`] to
    /// observe completion, which typically lands within ~100ms once AX publishes
    /// the resulting geometry change.
    pub async fn request_place_grid(
        &self,
        intent: PlaceIntent,
    ) -> Result<CommandReceipt, CommandError> {
        self.dispatch_operation(OperationRequest::PlaceGrid(intent))
            .await
    }

    /// Queue a grid placement command for a specific window.
    pub async fn request_place_for_window(
        &self,
        target: WorldWindowId,
        cols: u32,
        rows: u32,
        col: u32,
        row: u32,
        options: Option<PlaceAttemptOptions>,
    ) -> Result<CommandReceipt, CommandError> {
        let intent = PlaceIntent {
            cols,
            rows,
            col,
            row,
            pid_hint: Some(target.pid()),
            target: Some(target),
            options,
        };
        self.request_place_grid(intent).await
    }

    /// Queue a relative move command on the placement grid.
    ///
    /// See [`WorldHandle::request_place_grid`] for completion semantics.
    pub async fn request_place_move_grid(
        &self,
        intent: MoveIntent,
    ) -> Result<CommandReceipt, CommandError> {
        self.dispatch_operation(OperationRequest::PlaceMoveGrid(intent))
            .await
    }

    /// Queue a relative move for a specific window identified by world id.
    pub async fn request_place_move_for_window(
        &self,
        target: WorldWindowId,
        cols: u32,
        rows: u32,
        dir: MoveDirection,
        options: Option<PlaceAttemptOptions>,
    ) -> Result<CommandReceipt, CommandError> {
        let intent = MoveIntent {
            cols,
            rows,
            dir,
            pid_hint: Some(target.pid()),
            target: Some(target),
            options,
        };
        self.request_place_move_grid(intent).await
    }

    /// Queue a hide/show request for the active application.
    ///
    /// Completion is observable via AX-backed [`WorldEvent`] updates on the
    /// application's windows.
    pub async fn request_hide(&self, intent: HideIntent) -> Result<CommandReceipt, CommandError> {
        self.dispatch_operation(OperationRequest::Hide(intent))
            .await
    }

    /// Queue a fullscreen request for the active application.
    ///
    /// AX updates typically surface within ~150ms once macOS transitions the
    /// window; subscribe to [`WorldEvent::Updated`] for confirmation.
    pub async fn request_fullscreen(
        &self,
        intent: FullscreenIntent,
    ) -> Result<CommandReceipt, CommandError> {
        self.dispatch_operation(OperationRequest::Fullscreen(intent))
            .await
    }

    /// Queue a raise request based on optional regex filters.
    ///
    /// The receipt carries the selected window; monitor world events for the
    /// corresponding [`WorldWindowId`] to confirm foreground arrival.
    pub async fn request_raise(&self, intent: RaiseIntent) -> Result<CommandReceipt, CommandError> {
        self.dispatch_operation(OperationRequest::Raise(intent))
            .await
    }

    /// Request focus navigation in the given direction.
    pub async fn request_focus_dir(
        &self,
        dir: MoveDirection,
    ) -> Result<CommandReceipt, CommandError> {
        self.dispatch_operation(OperationRequest::FocusDir(dir))
            .await
    }

    async fn dispatch_operation(
        &self,
        request: OperationRequest,
    ) -> Result<CommandReceipt, CommandError> {
        let (tx, rx) = oneshot::channel();
        if self
            .tx
            .send(Command::Operation {
                request,
                respond: tx,
            })
            .is_err()
        {
            return Err(CommandError::InvalidRequest {
                message: "World service is no longer running".to_string(),
            });
        }
        rx.await.unwrap_or_else(|_| {
            Err(CommandError::InvalidRequest {
                message: "World service dropped command response".to_string(),
            })
        })
    }

    /// Subscribe to world events and return an initial focus context, if any.
    ///
    /// The seed context is derived atomically relative to the returned
    /// snapshot+focused pair, but exposed here as a concise tuple to simplify
    /// downstream consumers.
    pub async fn subscribe_with_context(&self) -> (EventCursor, Option<(String, String, i32)>) {
        let (cursor, snap, focused) = self.subscribe_with_snapshot().await;
        let ctx = if let Some(fk) = focused {
            snap.iter()
                .find(|w| w.pid == fk.pid && w.id == fk.id)
                .map(|w| (w.app.clone(), w.title.clone(), w.pid))
        } else {
            snap.iter()
                .min_by_key(|w| w.z)
                .map(|w| (w.app.clone(), w.title.clone(), w.pid))
        };
        (cursor, ctx)
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
mod events;
mod frames;
#[cfg(feature = "world-mimic")]
pub mod mimic;
mod view;
mod wait;

pub use events::{EventCursor, EventFilter, EventRecord};
pub use frames::{
    FrameKind, Frames, RectDelta, RectPx, WindowMode, default_eps, reconcile_authoritative,
};
pub use wait::{VisibilityPolicy, WaitConfig, WaitError, WindowObserver};

#[derive(Debug, Default)]
struct WorldMetrics {
    debounce_pending: AtomicUsize,
    suspects_pending: AtomicUsize,
    windows_count: AtomicUsize,
    reconcile_seq: AtomicU64,
}

impl WorldMetrics {
    fn snapshot(&self) -> WorldMetricsSnapshot {
        WorldMetricsSnapshot {
            windows_count: self.windows_count.load(Ordering::SeqCst),
            debounce_pending: self.debounce_pending.load(Ordering::SeqCst),
            suspects_pending: self.suspects_pending.load(Ordering::SeqCst),
            reconcile_seq: self.reconcile_seq.load(Ordering::SeqCst),
        }
    }

    fn sync_from_state(&self, state: &WorldState) {
        self.debounce_pending
            .store(state.coalesce.len(), Ordering::SeqCst);
        self.suspects_pending
            .store(state.suspects.len(), Ordering::SeqCst);
        self.windows_count
            .store(state.store.len(), Ordering::SeqCst);
        self.reconcile_seq.store(state.seen_seq, Ordering::SeqCst);
    }
}

pub use view::{TestWorld, WorldView};

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
        let events = Arc::new(EventHub::new(cfg.events_buffer.max(8)));

        let state = WorldState::new();
        let metrics = Arc::new(WorldMetrics::default());

        let handle = WorldHandle {
            tx: tx.clone(),
            events: events.clone(),
            metrics: metrics.clone(),
        };

        // Initialize the per‑PID AX read pool and give it a handle to nudge
        // the world actor when reads complete.
        ax_read_pool::init(handle.tx.clone());

        tokio::spawn(run_actor(
            rx,
            events.clone(),
            metrics.clone(),
            state,
            winops,
            cfg.clone(),
        ));

        // Bridge macOS AX observer events into world refresh hints with light
        // throttling to coalesce bursts (e.g., AXTitleChanged storms).
        // Throttle window: 16ms; send immediately if idle longer than that.
        if ax_bridge_enabled() {
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
        }

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
        let events = Arc::new(EventHub::new(DEFAULT_EVENT_CAPACITY));
        tokio::spawn(async move {
            while let Some(cmd) = rx.recv().await {
                match cmd {
                    Command::Snapshot { respond } => {
                        let _ = respond.send(Vec::new());
                    }
                    Command::FramesSnapshot { respond } => {
                        let _ = respond.send(HashMap::new());
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
                    Command::Operation { respond, .. } => {
                        let _ = respond.send(Err(CommandError::InvalidRequest {
                            message: "noop world does not execute commands".to_string(),
                        }));
                    }
                }
            }
        });
        WorldHandle {
            tx,
            events,
            metrics: Arc::new(WorldMetrics::default()),
        }
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
    frames: HashMap<WindowKey, Frames>,
    frame_history: HashMap<WindowKey, FrameHistory>,
    focused: Option<WindowKey>,
    pending_focus: Option<PendingFocus>,
    capabilities: Capabilities,
    seen_seq: u64,
    /// Pending coalesced Updated events keyed by window id.
    coalesce: HashMap<WindowKey, PendingUpdate>,
    last_tick_ms: u64,
    current_poll_ms: u64,
    warned_ax: bool,
    warned_screen: bool,
    /// Windows that have gone missing recently and are pending confirmation
    /// before eviction. Value is the number of consecutive misses observed.
    suspects: HashMap<WindowKey, u8>,
    /// Monotonic identifier generator for world commands.
    next_command_id: CommandId,
    /// Last hidden targets keyed by pid for hide toggles.
    hidden_targets: HashMap<i32, WorldWindowId>,
    /// Most recently hidden window identifier, used for toggle semantics.
    last_hidden_target: Option<WorldWindowId>,
    /// Pending hide operations awaiting the window entering Hidden mode.
    pending_hide: HashSet<WorldWindowId>,
}

impl WorldState {
    fn new() -> Self {
        Self {
            store: HashMap::new(),
            frames: HashMap::new(),
            frame_history: HashMap::new(),
            focused: None,
            pending_focus: None,
            capabilities: Capabilities::default(),
            seen_seq: 0,
            coalesce: HashMap::new(),
            last_tick_ms: 0,
            current_poll_ms: 0,
            warned_ax: false,
            warned_screen: false,
            suspects: HashMap::new(),
            next_command_id: 1,
            hidden_targets: HashMap::new(),
            last_hidden_target: None,
            pending_hide: HashSet::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct FrameHistory {
    last_authoritative: RectPx,
    last_unminimized: Option<RectPx>,
}

#[derive(Clone, Debug)]
struct PendingUpdate {
    due: Instant,
    delta: WindowDelta,
}

impl PendingUpdate {
    fn new(now: Instant) -> Self {
        Self {
            due: schedule_deadline(now),
            delta: WindowDelta::default(),
        }
    }

    fn reschedule(&mut self, now: Instant) {
        self.due = schedule_deadline(now);
    }
}

#[derive(Clone, Copy, Debug)]
struct PendingFocus {
    key: WindowKey,
    expires_at: Instant,
}

impl PendingFocus {
    const TTL: Duration = Duration::from_millis(1_500);

    fn new(key: WindowKey, now: Instant) -> Self {
        Self {
            key,
            expires_at: now + Self::TTL,
        }
    }

    fn is_expired(&self, now: Instant) -> bool {
        now >= self.expires_at
    }
}

impl WorldState {
    fn queue_update(&mut self, key: WindowKey, delta: WindowDelta, now: Instant) {
        let entry = self
            .coalesce
            .entry(key)
            .or_insert_with(|| PendingUpdate::new(now));
        entry.delta.merge(delta);
        entry.reschedule(now);
    }

    fn expect_focus(&mut self, key: WindowKey) {
        let now = Instant::now();
        self.pending_focus = Some(PendingFocus::new(key, now));
    }
}

enum Command {
    Snapshot {
        respond: oneshot::Sender<Vec<WorldWindow>>,
    },
    FramesSnapshot {
        respond: oneshot::Sender<HashMap<WindowKey, Frames>>,
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
    /// Execute a world-orchestrated command (placement, hide, etc.).
    Operation {
        request: OperationRequest,
        respond: oneshot::Sender<Result<CommandReceipt, CommandError>>,
    },
}

#[derive(Debug)]
enum OperationRequest {
    PlaceGrid(PlaceIntent),
    PlaceMoveGrid(MoveIntent),
    Hide(HideIntent),
    Fullscreen(FullscreenIntent),
    Raise(RaiseIntent),
    FocusDir(MoveDirection),
}

async fn run_actor(
    mut rx: mpsc::UnboundedReceiver<Command>,
    events: Arc<EventHub>,
    metrics: Arc<WorldMetrics>,
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
    metrics.sync_from_state(&state);
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
                    Command::FramesSnapshot { respond } => {
                        let _ = respond.send(state.frames.clone());
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
                            debounce_cache: state.coalesce.len(),
                            debounce_pending: state.coalesce.len(),
                            reconcile_seq: state.seen_seq,
                            suspects_pending: state.suspects.len(),
                            capabilities: state.capabilities.clone(),
                        };
                        let _ = respond.send(status);
                    }
                    Command::Operation { request, respond } => {
                        let result = process_operation(&mut state, &*winops, request);
                        let ok = result.is_ok();
                        let _ = respond.send(result);
                        if ok {
                            current_ms = cfg.poll_ms_min;
                            state.current_poll_ms = current_ms;
                            next_tick.as_mut().reset(TokioInstant::now());
                        }
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
                metrics.sync_from_state(&state);

                // Update coalesce timer to earliest pending deadline
                next_coalesce_due = state.coalesce.values().map(|p| p.due).min();
                if let Some(due) = next_coalesce_due {
                    coalesce_tick.as_mut().reset(TokioInstant::from_std(due));
                }
            }
            _ = &mut coalesce_tick => {
                let now = Instant::now();
                // Emit coalesced updates whose deadline has passed
                let ready_keys: Vec<WindowKey> = state
                    .coalesce
                    .iter()
                    .filter_map(|(&k, pending)| if pending.due <= now { Some(k) } else { None })
                    .collect();
                for key in ready_keys {
                    if let Some(pending) = state.coalesce.remove(&key)
                        && state.store.contains_key(&key) {
                            events.publish(WorldEvent::Updated(key, pending.delta));
                        }
                }
                metrics.sync_from_state(&state);
                // Re-arm the timer for the next earliest deadline
                next_coalesce_due = state.coalesce.values().map(|p| p.due).min();
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

fn reconcile(state: &mut WorldState, events: &EventHub, winops: &dyn WinOps) -> bool {
    let now = Instant::now();
    state.seen_seq = state.seen_seq.wrapping_add(1);
    let seq = state.seen_seq;

    let wins: Vec<WindowInfo> = winops.list_windows_for_spaces(&[]);
    let mut had_changes = false;

    let mut pending_focus_key = None;
    if let Some(pending) = state.pending_focus {
        if pending.is_expired(now) {
            state.pending_focus = None;
        } else if wins
            .iter()
            .any(|w| w.pid == pending.key.pid && w.id == pending.key.id)
        {
            pending_focus_key = Some(pending.key);
        }
    }

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

    const AX_FOCUS_ACCEPT_MAX_INDEX: usize = 16;

    if let Some(pid) = front_pid
        && acc_ok()
        && let Some(ax_id) = ax_read_pool::focused_id(pid)
    {
        let candidate = WindowKey { pid, id: ax_id };
        if let Some((index, info)) = wins
            .iter()
            .enumerate()
            .find(|(_, w)| w.pid == pid && w.id == ax_id)
        {
            let matches_expectation = pending_focus_key == Some(candidate);
            let cg_consistent = if matches_expectation {
                true
            } else {
                info.layer == 0
                    && info.is_on_screen
                    && (info.focused || index <= AX_FOCUS_ACCEPT_MAX_INDEX)
            };
            if cg_consistent {
                ax_focus_title = ax_read_pool::title(pid, ax_id);
                ax_focus_key = Some(candidate);
                tracing::debug!(
                    pid,
                    ax_id,
                    title = ax_focus_title.as_deref().unwrap_or(""),
                    index,
                    "reconcile: accepted ax focus candidate"
                );
            } else {
                tracing::debug!(
                    pid,
                    ax_id,
                    layer = info.layer,
                    is_on_screen = info.is_on_screen,
                    on_active_space = info.on_active_space,
                    focused = info.focused,
                    index,
                    "reconcile: rejected ax focus candidate due to cg mismatch"
                );
                ax_read_pool::invalidate_focus_silent(pid);
            }
        } else {
            tracing::debug!(
                pid,
                ax_id,
                "reconcile: rejected ax focus candidate absent from cg list"
            );
            ax_read_pool::invalidate_focus_silent(pid);
        }
    }

    let new_focused = pending_focus_key.or(ax_focus_key).or(cg_focus_key);

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
        let ax_props = ax_read_pool::props(w.pid, w.id);
        let cg_rect = w.pos.map(|pos| RectPx::from_pos(&pos));
        let ax_rect = ax_props
            .as_ref()
            .and_then(|props| props.frame.as_ref())
            .map(RectPx::from_ax);
        let last_unminimized = state
            .frame_history
            .get(&key)
            .and_then(|hist| hist.last_unminimized);
        let mode = determine_window_mode(w, ax_props.as_ref());
        let (authoritative, authoritative_kind) =
            reconcile_authoritative(ax_rect, cg_rect, mode, last_unminimized);
        let scale = resolve_display_scale(cg_rect, ax_rect, authoritative);
        let frames = Frames {
            authoritative,
            authoritative_kind,
            #[cfg(feature = "test-introspection")]
            ax: ax_rect,
            #[cfg(feature = "test-introspection")]
            cg: cg_rect,
            display_id,
            space_id: w.space,
            scale,
            mode,
        };
        let world_id = WorldWindowId::new(w.pid, w.id);
        if mode == WindowMode::Hidden {
            state.pending_hide.remove(&world_id);
        } else if !state.pending_hide.contains(&world_id) {
            if state.hidden_targets.get(&w.pid) == Some(&world_id) {
                tracing::trace!(
                    pid = w.pid,
                    id = w.id,
                    "reconcile: clearing hidden target for pid"
                );
                state.hidden_targets.remove(&w.pid);
            }
            if state.last_hidden_target == Some(world_id) {
                tracing::trace!(
                    pid = w.pid,
                    id = w.id,
                    "reconcile: clearing last hidden target"
                );
                state.last_hidden_target = None;
            }
        }
        if let Some(existing) = state.store.get_mut(&key) {
            let mut delta = WindowDelta::default();
            let new_title = if is_focus {
                ax_focus_title.clone().unwrap_or_else(|| w.title.clone())
            } else {
                w.title.clone()
            };
            if existing.title != new_title {
                let old = existing.title.clone();
                existing.title = new_title.clone();
                delta.title = Some(ValueChange::new(old, new_title));
            }
            if existing.layer != w.layer {
                let old = existing.layer;
                existing.layer = w.layer;
                delta.layer = Some(ValueChange::new(old, w.layer));
            }
            if existing.pos != w.pos {
                let old = existing.pos;
                let new_pos = w.pos;
                existing.pos = new_pos;
                delta.pos = Some(ValueChange::new(old, new_pos));
            }
            if existing.z != z {
                let old = existing.z;
                existing.z = z;
                delta.z = Some(ValueChange::new(old, z));
            }
            if existing.space != w.space {
                let old = existing.space;
                let new_space = w.space;
                existing.space = new_space;
                delta.space = Some(ValueChange::new(old, new_space));
            }
            if existing.on_active_space != w.on_active_space {
                let old = existing.on_active_space;
                existing.on_active_space = w.on_active_space;
                delta.on_active_space = Some(ValueChange::new(old, w.on_active_space));
            }
            if existing.is_on_screen != w.is_on_screen {
                let old = existing.is_on_screen;
                existing.is_on_screen = w.is_on_screen;
                delta.is_on_screen = Some(ValueChange::new(old, w.is_on_screen));
            }
            if existing.display_id != display_id {
                let old = existing.display_id;
                existing.display_id = display_id;
                delta.display_id = Some(ValueChange::new(old, display_id));
            }
            if existing.focused != is_focus {
                let old = existing.focused;
                existing.focused = is_focus;
                delta.focused = Some(ValueChange::new(old, is_focus));
            }
            // Populate AX props only for the focused window; clear otherwise.
            existing.ax = if is_focus { ax_props.clone() } else { None };
            existing.last_seen = now;
            existing.seen_seq = seq;
            if !delta.is_empty() {
                had_changes = true;
                state.queue_update(key, delta, now);
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
                ax: if is_focus { ax_props.clone() } else { None },
                meta: Vec::new(),
                last_seen: now,
                seen_seq: seq,
            };
            state.store.insert(key, ww.clone());
            events.publish(WorldEvent::Added(Box::new(ww)));
        }

        {
            let entry = state.frame_history.entry(key).or_default();
            entry.last_authoritative = frames.authoritative;
            if frames.mode != WindowMode::Minimized
                && frames.mode.is_visible()
                && frames.authoritative.w > 0
                && frames.authoritative.h > 0
            {
                entry.last_unminimized = Some(frames.authoritative);
            }
        }
        state.frames.insert(key, frames.clone());
        #[cfg(feature = "test-introspection")]
        if let (Some(ax_rect), Some(cg_rect)) = (frames.ax, frames.cg) {
            let delta = ax_rect.delta(&cg_rect);
            if delta != RectDelta::default() {
                tracing::debug!(
                    pid = key.pid,
                    id = key.id,
                    mode = ?frames.mode,
                    source = ?frames.authoritative_kind,
                    %delta,
                    "frame delta ax↔cg"
                );
            }
        }
    }

    // Removals with suspect confirmation
    const SUSPECT_MISSES: u8 = 1; // mark suspect after 1 missed pass; evict on next if still absent
    let seen: std::collections::HashSet<_> = seen_keys.iter().copied().collect();
    let existing_keys: Vec<_> = state.store.keys().copied().collect();
    let mut confirm_cache: Option<Vec<WindowInfo>> = None;
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
                let confirm =
                    confirm_cache.get_or_insert_with(|| winops.list_windows_for_spaces(&[]));
                let still_absent = !confirm.iter().any(|w| w.pid == key.pid && w.id == key.id);
                if still_absent {
                    had_changes = true;
                    let world_id = WorldWindowId::new(key.pid, key.id);
                    if state.hidden_targets.get(&key.pid) == Some(&world_id) {
                        tracing::trace!(
                            pid = key.pid,
                            id = key.id,
                            "reconcile: clearing hidden target on removal"
                        );
                        state.hidden_targets.remove(&key.pid);
                    }
                    if state.last_hidden_target == Some(world_id) {
                        tracing::trace!(
                            pid = key.pid,
                            id = key.id,
                            "reconcile: clearing last hidden target on removal"
                        );
                        state.last_hidden_target = None;
                    }
                    state.pending_hide.remove(&world_id);
                    mac_winops::clear_hidden_window(key.pid, key.id);
                    state.store.remove(&key);
                    state.coalesce.remove(&key);
                    state.suspects.remove(&key);
                    state.frames.remove(&key);
                    state.frame_history.remove(&key);
                    events.publish(WorldEvent::Removed(key));
                }
            }
        }
    }

    if let Some(pending) = state.pending_focus
        && Some(pending.key) == new_focused
    {
        state.pending_focus = None;
    }

    // Focus changes
    if state.focused != new_focused {
        state.focused = new_focused;
        let mut change = FocusChange {
            key: new_focused,
            app: None,
            title: None,
            pid: None,
        };
        if let Some(key) = new_focused
            && let Some(win) = state.store.get(&key)
        {
            change.app = Some(win.app.clone());
            change.title = Some(win.title.clone());
            change.pid = Some(win.pid);
        }
        events.publish(WorldEvent::FocusChanged(change));
    }

    had_changes
}

fn process_operation(
    state: &mut WorldState,
    winops: &dyn WinOps,
    request: OperationRequest,
) -> Result<CommandReceipt, CommandError> {
    match request {
        OperationRequest::PlaceGrid(intent) => handle_place_grid(state, winops, intent),
        OperationRequest::PlaceMoveGrid(intent) => handle_place_move(state, winops, intent),
        OperationRequest::Hide(intent) => handle_hide(state, winops, intent),
        OperationRequest::Fullscreen(intent) => handle_fullscreen(state, winops, intent),
        OperationRequest::Raise(intent) => handle_raise(state, winops, intent),
        OperationRequest::FocusDir(dir) => handle_focus_dir(state, winops, dir),
    }
}

fn handle_place_grid(
    state: &mut WorldState,
    winops: &dyn WinOps,
    intent: PlaceIntent,
) -> Result<CommandReceipt, CommandError> {
    let kind = CommandKind::PlaceGrid;
    let PlaceIntent {
        cols,
        rows,
        col,
        row,
        pid_hint,
        target,
        options,
    } = intent;

    let snapshot = sorted_snapshot(state);
    if snapshot.is_empty() {
        tracing::debug!("Place(World): empty snapshot; no-op");
        return Err(CommandError::NoEligibleWindow {
            kind,
            pid: pid_hint,
        });
    }

    if let Some(target_id) = target {
        let mut target_window = snapshot
            .iter()
            .find(|w| w.pid == target_id.pid() && w.id == target_id.window_id())
            .cloned()
            .ok_or_else(|| CommandError::InvalidRequest {
                message: format!(
                    "Target window pid={} id={} not present in world snapshot",
                    target_id.pid(),
                    target_id.window_id()
                ),
            })?;

        if !target_window.on_active_space || !target_window.is_on_screen {
            tracing::debug!(
                pid = target_window.pid,
                id = target_window.id,
                on_active_space = target_window.on_active_space,
                is_on_screen = target_window.is_on_screen,
                "Place(World): target off active space prior to refresh"
            );
            let listing = winops.list_windows_for_spaces(&[]);
            if let Some(fresh) = listing
                .into_iter()
                .find(|w| w.pid == target_window.pid && w.id == target_window.id)
                && fresh.on_active_space
                && fresh.is_on_screen
            {
                tracing::debug!(
                    pid = fresh.pid,
                    id = fresh.id,
                    "Place(World): refreshed target from CG listing to active space"
                );
                target_window.on_active_space = true;
                target_window.is_on_screen = true;
                target_window.space = fresh.space;
                target_window.layer = fresh.layer;
            }
        }

        placement_mode_guard(
            state,
            WindowKey {
                pid: target_window.pid,
                id: target_window.id,
            },
            kind,
        )?;

        if !target_window.on_active_space {
            tracing::debug!(
                "Place(World): explicit target off active space pid={} title='{}'",
                target_window.pid,
                target_window.title
            );
            let _ = winops.request_raise_window(target_window.pid, target_window.id);
        }

        let _ = winops.request_activate_pid(target_window.pid);
        let place_opts = options.clone().unwrap_or_default();
        let result =
            winops.request_place_grid_opts(target_id, cols, rows, col, row, place_opts.clone());

        if let Err(e) = result {
            if let WinOpsError::FocusedWindow = e {
                tracing::debug!(
                    pid = target_window.pid,
                    id = target_window.id,
                    "Place(World): id-based placement missing AX window; retrying focused path"
                );
                let fallback = winops.request_place_grid_focused_opts(
                    target_window.pid,
                    cols,
                    rows,
                    col,
                    row,
                    place_opts,
                );
                if let Err(focused_err) = fallback {
                    tracing::warn!(
                        error = %focused_err,
                        pid = target_window.pid,
                        id = target_window.id,
                        "Place(World): focused fallback failure"
                    );
                    return Err(CommandError::backend(kind, focused_err));
                }
                tracing::debug!(
                    pid = target_window.pid,
                    id = target_window.id,
                    "Place(World): focused fallback succeeded"
                );
            } else {
                tracing::warn!(
                    error = %e,
                    pid = target_window.pid,
                    id = target_window.id,
                    "Place(World): backend failure (explicit)"
                );
                return Err(CommandError::backend(kind, e));
            }
        }

        return Ok(issue_receipt(
            state,
            kind,
            Some(target_window),
            Some(TargetSelection::Explicit),
        ));
    }

    let focused = focused_window(state);
    let pid = determine_pid(pid_hint, focused.as_ref(), &snapshot).ok_or(
        CommandError::NoEligibleWindow {
            kind,
            pid: pid_hint,
        },
    )?;

    if let Some(ref wf) = focused
        && wf.pid == pid
        && !wf.on_active_space
    {
        return Err(off_active_space_error(pid, wf));
    }

    if let Some(off) = snapshot
        .iter()
        .find(|w| w.pid == pid && !w.on_active_space)
        .filter(|_| !snapshot.iter().any(|w| w.pid == pid && w.on_active_space))
    {
        return Err(off_active_space_error(pid, off));
    }

    if let Some(ref wf) = focused
        && wf.pid == pid
        && let Some(reason) = placement_guard_reason(wf)
    {
        tracing::debug!(
            "Place(World): skipped: reason={} app='{}' title='{}' pid={}",
            reason,
            wf.app,
            wf.title,
            wf.pid
        );
        return Ok(issue_receipt(state, kind, None, None));
    }

    let (target_window, selection) = resolve_target_for_pid(&snapshot, focused.as_ref(), pid)
        .ok_or(CommandError::NoEligibleWindow {
            kind,
            pid: Some(pid),
        })?;

    placement_mode_guard(
        state,
        WindowKey {
            pid: target_window.pid,
            id: target_window.id,
        },
        kind,
    )?;

    tracing::debug!(
        "Place(World): resolved via {} pid={} id={} app='{}' title='{}' cols={} rows={} col={} row={}",
        selection.as_str(),
        target_window.pid,
        target_window.id,
        target_window.app,
        target_window.title,
        cols,
        rows,
        col,
        row
    );

    let result = if let Some(opts) = options {
        winops.request_place_grid_opts(target_window.world_id(), cols, rows, col, row, opts)
    } else {
        winops.request_place_grid(target_window.world_id(), cols, rows, col, row)
    };

    if let Err(e) = result {
        tracing::warn!(
            error = %e,
            pid = target_window.pid,
            id = target_window.id,
            "Place(World): backend failure"
        );
        return Err(CommandError::backend(kind, e));
    }

    Ok(issue_receipt(
        state,
        kind,
        Some(target_window),
        Some(selection),
    ))
}

fn handle_place_move(
    state: &mut WorldState,
    winops: &dyn WinOps,
    intent: MoveIntent,
) -> Result<CommandReceipt, CommandError> {
    let kind = CommandKind::PlaceMoveGrid;
    let MoveIntent {
        cols,
        rows,
        dir,
        pid_hint,
        target,
        options,
    } = intent;

    let snapshot = sorted_snapshot(state);
    if snapshot.is_empty() {
        tracing::debug!("Move(World): empty snapshot; no-op");
        return Err(CommandError::NoEligibleWindow {
            kind,
            pid: pid_hint,
        });
    }

    if let Some(target_id) = target {
        let target_window = snapshot
            .iter()
            .find(|w| w.pid == target_id.pid() && w.id == target_id.window_id())
            .cloned()
            .ok_or_else(|| CommandError::InvalidRequest {
                message: format!(
                    "Target window pid={} id={} not present in world snapshot",
                    target_id.pid(),
                    target_id.window_id()
                ),
            })?;

        if !target_window.on_active_space {
            tracing::debug!(
                "Move(World): explicit target off active space pid={} title='{}'",
                target_window.pid,
                target_window.title
            );
        }

        placement_mode_guard(
            state,
            WindowKey {
                pid: target_window.pid,
                id: target_window.id,
            },
            kind,
        )?;

        let dir_mc = convert_move_dir(dir);
        let _ = winops.request_activate_pid(target_window.pid);
        let move_opts = options.clone().unwrap_or_default();
        let result =
            winops.request_place_move_grid_opts(target_id, cols, rows, dir_mc, move_opts.clone());

        if let Err(e) = result {
            tracing::warn!(
                error = %e,
                pid = target_window.pid,
                id = target_window.id,
                "Move(World): backend failure (explicit)"
            );
            return Err(CommandError::backend(kind, e));
        }

        return Ok(issue_receipt(
            state,
            kind,
            Some(target_window),
            Some(TargetSelection::Explicit),
        ));
    }

    let focused = focused_window(state);
    let pid = determine_pid(pid_hint, focused.as_ref(), &snapshot).ok_or(
        CommandError::NoEligibleWindow {
            kind,
            pid: pid_hint,
        },
    )?;

    if let Some(ref wf) = focused
        && wf.pid == pid
        && !wf.on_active_space
    {
        return Err(off_active_space_error(pid, wf));
    }

    if let Some(off) = snapshot
        .iter()
        .find(|w| w.pid == pid && !w.on_active_space)
        .filter(|_| !snapshot.iter().any(|w| w.pid == pid && w.on_active_space))
    {
        return Err(off_active_space_error(pid, off));
    }

    let (target_window, selection) = resolve_target_for_pid(&snapshot, focused.as_ref(), pid)
        .ok_or(CommandError::NoEligibleWindow {
            kind,
            pid: Some(pid),
        })?;

    placement_mode_guard(
        state,
        WindowKey {
            pid: target_window.pid,
            id: target_window.id,
        },
        kind,
    )?;

    if target_window.focused
        && let Some(reason) = placement_guard_reason(&target_window)
    {
        tracing::debug!(
            "Move(World): skipped: reason={} app='{}' title='{}' pid={}",
            reason,
            target_window.app,
            target_window.title,
            target_window.pid
        );
        return Ok(issue_receipt(state, kind, None, None));
    }

    let dir_mc = convert_move_dir(dir);
    tracing::debug!(
        "Move(World): resolved via {} pid={} id={} app='{}' title='{}' cols={} rows={} dir={:?}",
        selection.as_str(),
        target_window.pid,
        target_window.id,
        target_window.app,
        target_window.title,
        cols,
        rows,
        dir
    );

    let result = if let Some(opts) = options {
        winops.request_place_move_grid_opts(target_window.world_id(), cols, rows, dir_mc, opts)
    } else {
        winops.request_place_move_grid(target_window.world_id(), cols, rows, dir_mc)
    };

    if let Err(e) = result {
        tracing::warn!(
            error = %e,
            pid = target_window.pid,
            id = target_window.id,
            "Move(World): backend failure"
        );
        return Err(CommandError::backend(kind, e));
    }

    Ok(issue_receipt(
        state,
        kind,
        Some(target_window),
        Some(selection),
    ))
}

fn handle_hide(
    state: &mut WorldState,
    winops: &dyn WinOps,
    intent: HideIntent,
) -> Result<CommandReceipt, CommandError> {
    let kind = CommandKind::Hide;
    let snapshot = sorted_snapshot(state);
    let focused = focused_window(state);
    let mut pid =
        determine_pid(None, focused.as_ref(), &snapshot).ok_or(CommandError::InvalidRequest {
            message: "Hide requires an active application".to_string(),
        })?;

    let last_hidden = state.last_hidden_target;
    let will_hide = matches!(intent.desired, CommandToggle::On)
        || (matches!(intent.desired, CommandToggle::Toggle) && last_hidden.is_none());
    let will_show = matches!(intent.desired, CommandToggle::Off)
        || (matches!(intent.desired, CommandToggle::Toggle) && last_hidden.is_some());

    tracing::debug!(
        "Hide(World): pre-select pid={} desired={:?} focused_pid={:?} last_hidden_pid={:?}",
        pid,
        intent.desired,
        focused.as_ref().map(|w| w.pid),
        last_hidden.map(|id| id.pid()),
    );

    if will_show && let Some(hidden_id) = last_hidden {
        pid = hidden_id.pid();
    }

    let desired = convert_toggle(intent.desired);
    let stored_hidden = state.hidden_targets.get(&pid).copied();
    let mut hidden_receipt = stored_hidden;

    tracing::debug!(
        "Hide(World): post-select pid={} stored_hidden_pid={:?}",
        pid,
        stored_hidden.map(|id| id.pid()),
    );

    if let Some(top) = snapshot.first() {
        tracing::debug!(
            "Hide(World): pid={} desired={:?} focus_app='{}' focus_title='{}' top_pid={} top_id={} top_app='{}' top_title='{}'",
            pid,
            intent.desired,
            focused.as_ref().map(|w| w.app.as_str()).unwrap_or(""),
            focused.as_ref().map(|w| w.title.as_str()).unwrap_or(""),
            top.pid,
            top.id,
            top.app,
            top.title
        );
    } else {
        tracing::debug!(
            "Hide(World): pid={} desired={:?} focus_app='{}' focus_title='{}' top=<none>",
            pid,
            intent.desired,
            focused.as_ref().map(|w| w.app.as_str()).unwrap_or(""),
            focused.as_ref().map(|w| w.title.as_str()).unwrap_or("")
        );
    }

    let mut target_window = focused.clone();
    if will_show && hidden_receipt.is_none() && last_hidden.is_some() {
        hidden_receipt = last_hidden;
    }
    if will_show
        && let Some(hidden_id) = hidden_receipt
        && let Some(found) = snapshot.iter().find(|w| w.world_id() == hidden_id)
    {
        target_window = Some(found.clone());
    }

    if let Err(e) = winops.hide_bottom_left(pid, desired) {
        tracing::warn!(error = %e, pid, "Hide(World): backend failure");
        return Err(CommandError::backend(kind, e));
    }

    if will_hide {
        if let Some(ref win) = target_window {
            let world_id = win.world_id();
            state.pending_hide.insert(world_id);
            state.hidden_targets.insert(pid, world_id);
            state.last_hidden_target = Some(world_id);
        }
    } else if will_show {
        if let Some(hidden_id) = hidden_receipt {
            state.pending_hide.remove(&hidden_id);
        }
        state.hidden_targets.remove(&pid);
        if state.last_hidden_target.is_some_and(|id| id.pid() == pid) {
            state.last_hidden_target = None;
        }
    }

    Ok(issue_receipt(state, kind, target_window, None))
}

fn handle_fullscreen(
    state: &mut WorldState,
    winops: &dyn WinOps,
    intent: FullscreenIntent,
) -> Result<CommandReceipt, CommandError> {
    let kind = CommandKind::Fullscreen;
    let snapshot = sorted_snapshot(state);
    let focused = focused_window(state);
    let pid =
        determine_pid(None, focused.as_ref(), &snapshot).ok_or(CommandError::InvalidRequest {
            message: "Fullscreen requires an active application".to_string(),
        })?;

    let desired = convert_toggle(intent.desired);
    if let Some(top) = snapshot.first() {
        tracing::debug!(
            "Fullscreen(World): pid={} desired={:?} kind={:?} focus_app='{}' focus_title='{}' top_pid={} top_id={} top_app='{}' top_title='{}'",
            pid,
            intent.desired,
            intent.kind,
            focused.as_ref().map(|w| w.app.as_str()).unwrap_or(""),
            focused.as_ref().map(|w| w.title.as_str()).unwrap_or(""),
            top.pid,
            top.id,
            top.app,
            top.title
        );
    } else {
        tracing::debug!(
            "Fullscreen(World): pid={} desired={:?} kind={:?} focus_app='{}' focus_title='{}' top=<none>",
            pid,
            intent.desired,
            intent.kind,
            focused.as_ref().map(|w| w.app.as_str()).unwrap_or(""),
            focused.as_ref().map(|w| w.title.as_str()).unwrap_or("")
        );
    }

    let res = match intent.kind {
        FullscreenKind::Native => winops.request_fullscreen_native(pid, desired),
        FullscreenKind::Nonnative => winops.request_fullscreen_nonnative(pid, desired),
    };

    if let Err(e) = res {
        tracing::warn!(error = %e, pid, kind = ?intent.kind, "Fullscreen(World): backend failure");
        return Err(CommandError::backend(kind, e));
    }

    Ok(issue_receipt(state, kind, focused, None))
}

fn handle_raise(
    state: &mut WorldState,
    winops: &dyn WinOps,
    intent: RaiseIntent,
) -> Result<CommandReceipt, CommandError> {
    let kind = CommandKind::Raise;
    let snapshot = sorted_snapshot(state);
    if snapshot.is_empty() {
        tracing::debug!("Raise(World): empty snapshot; no-op");
        return Err(CommandError::NoEligibleWindow { kind, pid: None });
    }
    let focused = focused_window(state);

    let mut idx_any: Vec<usize> = Vec::new();
    let mut idx_active: Vec<usize> = Vec::new();
    for (idx, window) in snapshot.iter().enumerate() {
        if intent.matches(window) {
            idx_any.push(idx);
            if window.on_active_space {
                idx_active.push(idx);
            }
        }
    }

    tracing::debug!(
        "Raise(World): matched active={} total={}",
        idx_active.len(),
        idx_any.len()
    );

    let pick_index = |candidates: &[usize]| -> Option<usize> {
        if candidates.is_empty() {
            return None;
        }
        if let Some(ref focused_window) = focused
            && intent.matches(focused_window)
            && let Some(cur_index) = snapshot
                .iter()
                .position(|w| w.pid == focused_window.pid && w.id == focused_window.id)
            && let Some(next) = candidates.iter().copied().find(|&i| i > cur_index)
        {
            return Some(next);
        }
        candidates.first().copied()
    };

    if let Some(target_idx) = pick_index(&idx_active).or_else(|| pick_index(&idx_any)) {
        let target = snapshot[target_idx].clone();
        tracing::debug!(
            "Raise(World): target pid={} id={} app='{}' title='{}' off_space={}",
            target.pid,
            target.id,
            target.app,
            target.title,
            !target.on_active_space
        );

        if !winops.ensure_frontmost_by_title(target.pid, &target.title, 7, 80) {
            tracing::warn!(
                pid = target.pid,
                id = target.id,
                "Raise(World): ensure_frontmost_by_title fallback"
            );
            if let Err(e) = winops.request_activate_pid(target.pid) {
                tracing::warn!(error = %e, pid = target.pid, "Raise(World): activate fallback failed");
                return Err(CommandError::backend(kind, e));
            }
        }

        let focus_key = WindowKey {
            pid: target.pid,
            id: target.id,
        };
        state.expect_focus(focus_key);
        ax_read_pool::invalidate_focus(target.pid);

        Ok(issue_receipt(
            state,
            kind,
            Some(target),
            Some(TargetSelection::Cycle),
        ))
    } else {
        tracing::debug!("Raise(World): no match in snapshot; no-op");
        Err(CommandError::NoEligibleWindow { kind, pid: None })
    }
}

fn handle_focus_dir(
    state: &mut WorldState,
    winops: &dyn WinOps,
    dir: MoveDirection,
) -> Result<CommandReceipt, CommandError> {
    let kind = CommandKind::FocusDir;
    tracing::debug!("Focus(World): request dir={:?}", dir);
    let mac_dir = convert_move_dir(dir);
    if let Err(e) = winops.request_focus_dir(mac_dir) {
        tracing::warn!(error = %e, "Focus(World): backend failure");
        return Err(CommandError::backend(kind, e));
    }

    let target = focused_window(state);
    let selection = target.as_ref().map(|_| TargetSelection::Focused);
    Ok(issue_receipt(state, kind, target, selection))
}

fn determine_pid(
    hint: Option<i32>,
    focused: Option<&WorldWindow>,
    snapshot: &[WorldWindow],
) -> Option<i32> {
    if let Some(pid) = hint {
        return Some(pid);
    }
    if let Some(focused) = focused {
        return Some(focused.pid);
    }
    snapshot
        .iter()
        .find(|w| w.on_active_space)
        .or_else(|| snapshot.first())
        .map(|w| w.pid)
}

fn sorted_snapshot(state: &WorldState) -> Vec<WorldWindow> {
    let mut windows: Vec<_> = state.store.values().cloned().collect();
    windows.sort_by_key(|w| (w.z, w.pid, w.id));
    windows
}

fn focused_window(state: &WorldState) -> Option<WorldWindow> {
    state.focused.and_then(|key| state.store.get(&key).cloned())
}

fn resolve_target_for_pid(
    snapshot: &[WorldWindow],
    focused: Option<&WorldWindow>,
    pid: i32,
) -> Option<(WorldWindow, TargetSelection)> {
    if let Some(focused_window) = focused
        && focused_window.pid == pid
        && focused_window.on_active_space
    {
        return Some((focused_window.clone(), TargetSelection::Focused));
    }

    snapshot
        .iter()
        .filter(|w| w.pid == pid && w.on_active_space)
        .min_by_key(|w| (w.z, w.id))
        .cloned()
        .map(|w| (w, TargetSelection::ActiveFrontmost))
}

fn issue_receipt(
    state: &mut WorldState,
    kind: CommandKind,
    target: Option<WorldWindow>,
    selection: Option<TargetSelection>,
) -> CommandReceipt {
    let id = state.next_command_id;
    state.next_command_id = state.next_command_id.wrapping_add(1).max(1);
    CommandReceipt {
        id,
        kind,
        issued_at: Instant::now(),
        target,
        selection,
    }
}

fn convert_toggle(toggle: CommandToggle) -> mac_winops::Desired {
    match toggle {
        CommandToggle::On => mac_winops::Desired::On,
        CommandToggle::Off => mac_winops::Desired::Off,
        CommandToggle::Toggle => mac_winops::Desired::Toggle,
    }
}

fn convert_move_dir(dir: MoveDirection) -> mac_winops::MoveDir {
    match dir {
        MoveDirection::Left => mac_winops::MoveDir::Left,
        MoveDirection::Right => mac_winops::MoveDir::Right,
        MoveDirection::Up => mac_winops::MoveDir::Up,
        MoveDirection::Down => mac_winops::MoveDir::Down,
    }
}

fn placement_guard_reason(window: &WorldWindow) -> Option<&'static str> {
    let ax = window.ax.as_ref()?;
    let role = ax.role.as_deref().unwrap_or_default();
    let subrole = ax.subrole.as_deref().unwrap_or_default();
    if role == "AXSheet" {
        return Some("role=AXSheet");
    }
    if role == "AXPopover" || subrole == "AXPopover" {
        return Some("popover");
    }
    if subrole == "AXDialog" || subrole == "AXSystemDialog" {
        return Some("dialog");
    }
    if subrole == "AXFloatingWindow" {
        return Some("floating");
    }
    if ax.can_set_pos == Some(false) {
        return Some("not settable");
    }
    None
}

fn placement_mode_guard(
    state: &WorldState,
    key: WindowKey,
    kind: CommandKind,
) -> Result<(), CommandError> {
    let Some(frames) = state.frames.get(&key) else {
        return Ok(());
    };
    match frames.mode {
        WindowMode::Normal | WindowMode::Hidden => Ok(()),
        blocked => Err(CommandError::InvalidRequest {
            message: format!(
                "Cannot perform {:?} while window mode={:?}. Exit this mode or adjust placement options.",
                kind, blocked
            ),
        }),
    }
}

fn off_active_space_error(pid: i32, window: &WorldWindow) -> CommandError {
    CommandError::OffActiveSpace {
        pid,
        space: window.space,
    }
}

impl WorldHandle {
    /// Hint that the frontmost app/window likely changed; triggers immediate refresh.
    pub fn hint_refresh(&self) {
        let _ = self.tx.send(Command::HintRefresh);
    }
}

fn update_capabilities(state: &mut WorldState) {
    let ax_ok = acc_ok();
    let sr_ok = screen_ok();
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

fn determine_window_mode(info: &WindowInfo, ax: Option<&AxProps>) -> WindowMode {
    if mac_winops::is_window_hidden(info.pid, info.id) {
        let ax_visible = ax.and_then(|props| props.visible);
        let mimic_helper = info.app == "smoketest" && info.title.contains('[');
        let hidden_target = mac_winops::hidden_window_target(info.pid, info.id);
        let target_match = if let (Some(target), Some(pos)) = (hidden_target, info.pos) {
            let dx = (f64::from(pos.x) - target.x).abs();
            let dy = (f64::from(pos.y) - target.y).abs();
            dx <= 4.0 && dy <= 4.0
        } else {
            false
        };
        let hidden_frame = mac_winops::hidden_window_frame(info.pid, info.id);
        let offset_hidden =
            if let (Some((orig_pos, orig_size)), Some(pos)) = (hidden_frame, info.pos) {
                let dx = (f64::from(pos.x) - orig_pos.x).abs();
                let dy = (f64::from(pos.y) - orig_pos.y).abs();
                let half_w = (orig_size.width / 2.0).max(1.0);
                let half_h = (orig_size.height / 2.0).max(1.0);
                dx >= half_w || dy >= half_h
            } else {
                false
            };
        let cg_reported_hidden =
            !info.is_on_screen && !info.on_active_space && ax_visible != Some(true);
        let still_hidden = mimic_helper || target_match || offset_hidden || cg_reported_hidden;
        if still_hidden {
            tracing::trace!(
                pid = info.pid,
                id = info.id,
                mimic_helper,
                target_match,
                offset_hidden,
                cg_reported_hidden,
                "determine_window_mode: cached hidden state confirmed"
            );
            return WindowMode::Hidden;
        }
        tracing::trace!(
            pid = info.pid,
            id = info.id,
            mimic_helper,
            target_match,
            offset_hidden,
            cg_reported_hidden,
            target_x = hidden_target.map(|p| p.x),
            target_y = hidden_target.map(|p| p.y),
            pos_x = info.pos.map(|p| p.x),
            pos_y = info.pos.map(|p| p.y),
            orig_x = hidden_frame.map(|(p, _)| p.x),
            orig_y = hidden_frame.map(|(p, _)| p.y),
            orig_w = hidden_frame.map(|(_, s)| s.width),
            orig_h = hidden_frame.map(|(_, s)| s.height),
            "determine_window_mode: clearing stale hidden cache entry"
        );
        mac_winops::clear_hidden_window(info.pid, info.id);
    }
    if let Some(props) = ax {
        if props.minimized == Some(true) {
            return WindowMode::Minimized;
        }
        if props.fullscreen == Some(true) {
            // macOS split view windows still report AXFullScreen true but retain standard subrole.
            if props
                .subrole
                .as_deref()
                .is_some_and(|s| s.contains("Standard"))
            {
                return WindowMode::Tiled;
            }
            return WindowMode::Fullscreen;
        }
        if props.visible == Some(false) && !info.is_on_screen {
            return WindowMode::Hidden;
        }
    }
    if !info.is_on_screen && !info.on_active_space {
        return WindowMode::Hidden;
    }
    WindowMode::Normal
}

fn resolve_display_scale(cg: Option<RectPx>, ax: Option<RectPx>, authoritative: RectPx) -> f32 {
    let rect = cg.or(ax).unwrap_or(authoritative);
    let w = rect.w.max(1) as f64;
    let h = rect.h.max(1) as f64;
    let cx = rect.x as f64 + w / 2.0;
    let cy = rect.y as f64 + h / 2.0;
    screen::display_scale_containing_point(cx, cy)
        .unwrap_or(1.0)
        .max(1.0) as f32
}

// ===== AX and permission shims (overridable for tests) =====
use parking_lot::Mutex;
thread_local! { static TEST_OVERRIDES: Mutex<TestOverrides> = Mutex::new(TestOverrides::default()); }
static GLOBAL_TEST_OVERRIDES: OnceLock<Mutex<TestOverrides>> = OnceLock::new();

fn global_overrides() -> &'static Mutex<TestOverrides> {
    GLOBAL_TEST_OVERRIDES.get_or_init(|| Mutex::new(TestOverrides::default()))
}

fn override_value<T: Clone>(getter: impl Fn(&TestOverrides) -> Option<T>) -> Option<T> {
    if let Some(val) = TEST_OVERRIDES.with(|o| {
        let guard = o.lock();
        getter(&guard)
    }) {
        return Some(val);
    }
    let guard = global_overrides().lock();
    getter(&guard)
}

#[derive(Default, Clone)]
struct TestOverrides {
    acc_ok: Option<bool>,
    ax_focus: Option<(i32, u32)>,
    ax_title: Option<(u32, String)>,
    displays: Option<Vec<DisplayBounds>>,
    ax_delay_title_ms: Option<u64>,
    ax_delay_focus_ms: Option<u64>,
    ax_async_only: Option<bool>,
    coalesce_ms: Option<u64>,
    ax_bridge_enabled: Option<bool>,
    screen_ok: Option<bool>,
}

fn acc_ok() -> bool {
    if let Some(v) = override_value(|o| o.acc_ok) {
        return v;
    }
    permissions::accessibility_ok()
}

fn screen_ok() -> bool {
    if let Some(v) = override_value(|o| o.screen_ok) {
        return v;
    }
    permissions::screen_recording_ok()
}

fn ax_focused_window_id_for_pid(pid: i32) -> Option<u32> {
    // Optional test delay
    if let Some(ms) = override_value(|o| o.ax_delay_focus_ms) {
        std::thread::sleep(std::time::Duration::from_millis(ms));
    }
    if let Some((p, id)) = override_value(|o| o.ax_focus)
        && p == pid
    {
        return Some(id);
    }
    mac_winops::ax_focused_window_id_for_pid(pid)
}

fn ax_title_for_window_id(id: u32) -> Option<String> {
    // Optional test delay
    if let Some(ms) = override_value(|o| o.ax_delay_title_ms) {
        std::thread::sleep(std::time::Duration::from_millis(ms));
    }
    if let Some((tid, title)) = override_value(|o| o.ax_title.clone())
        && tid == id
    {
        return Some(title);
    }
    mac_winops::ax_title_for_window_id(id)
}

fn list_display_bounds() -> Vec<DisplayBounds> {
    if let Some(v) = override_value(|o| o.displays.clone()) {
        return v;
    }
    mac_winops::screen::list_display_bounds()
}

fn schedule_deadline(now: Instant) -> Instant {
    now + Duration::from_millis(coalesce_window_ms())
}

fn coalesce_window_ms() -> u64 {
    override_value(|o| o.coalesce_ms).unwrap_or(50).max(1)
}

fn ax_bridge_enabled() -> bool {
    override_value(|o| o.ax_bridge_enabled).unwrap_or(true)
}

#[doc(hidden)]
pub mod test_api {
    use std::time::Duration;

    use tokio::time::{Instant, sleep};

    use super::{AxProps, DisplayBounds, TEST_OVERRIDES, TestOverrides, WorldWindow};

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
        super::global_overrides().lock().acc_ok = Some(v);
    }
    pub fn set_screen_recording_ok(v: bool) {
        TEST_OVERRIDES.with(|o| {
            let mut s = o.lock();
            s.screen_ok = Some(v);
        });
        super::global_overrides().lock().screen_ok = Some(v);
    }
    pub fn set_ax_focus(pid: i32, id: u32) {
        TEST_OVERRIDES.with(|o| {
            let mut s = o.lock();
            s.ax_focus = Some((pid, id));
        });
        super::global_overrides().lock().ax_focus = Some((pid, id));
    }
    // set_ax_title now forwards to the AX read pool's cross-thread override so worker threads
    // observe the value. For legacy behavior (thread-local override), prefer using the private
    // helper inside unit tests.
    pub fn set_displays(v: Vec<DisplayBounds>) {
        let clone = v.clone();
        TEST_OVERRIDES.with(|o| {
            let mut s = o.lock();
            s.displays = Some(v);
        });
        super::global_overrides().lock().displays = Some(clone);
    }
    pub fn clear() {
        TEST_OVERRIDES.with(|o| {
            let mut s = o.lock();
            *s = TestOverrides::default();
        });
        *super::global_overrides().lock() = TestOverrides::default();
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
    pub fn ax_pool_cache_usage() -> (usize, usize) {
        super::ax_read_pool::_test_cache_usage()
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
        super::global_overrides().lock().ax_delay_focus_ms = Some(ms);
    }
    pub fn set_ax_async_only(v: bool) {
        TEST_OVERRIDES.with(|o| {
            let mut s = o.lock();
            s.ax_async_only = Some(v);
        });
        super::global_overrides().lock().ax_async_only = Some(v);
    }
    pub fn set_ax_bridge_enabled(v: bool) {
        TEST_OVERRIDES.with(|o| {
            let mut s = o.lock();
            s.ax_bridge_enabled = Some(v);
        });
        super::global_overrides().lock().ax_bridge_enabled = Some(v);
    }
    pub fn set_ax_title(id: u32, title: &str) {
        // Set both: thread-local (for synchronous override path) and
        // pool-level (so worker threads also observe it).
        let t = title.to_string();
        TEST_OVERRIDES.with(|o| {
            let mut s = o.lock();
            s.ax_title = Some((id, t));
        });
        super::global_overrides().lock().ax_title = Some((id, title.to_string()));
        super::ax_read_pool::_test_set_title_override(id, title);
    }
    pub fn set_ax_props(pid: i32, id: u32, props: AxProps) {
        super::ax_read_pool::_test_set_props_override(pid, id, props);
    }
    pub fn set_coalesce_ms(ms: u64) {
        TEST_OVERRIDES.with(|o| {
            let mut s = o.lock();
            s.coalesce_ms = Some(ms.max(1));
        });
        super::global_overrides().lock().coalesce_ms = Some(ms.max(1));
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
