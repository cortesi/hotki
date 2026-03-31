#![warn(missing_docs)]
#![warn(unsafe_op_in_unsafe_fn)]

//! Minimal focus + display snapshot service for Hotki.
//!
//! Stage 3 of the WinOps removal collapses `hotki-world` into a read-only
//! provider. The service now tracks only:
//! - focused app/title/pid context (best-effort)
//! - a lightweight window list
//! - display geometry snapshots
//!
//! There are no mutating commands (place/hide/focus/raise); callers should use
//! external tooling for window control. The exported surface is intentionally
//! small and stable: [`WorldView`] for querying state, [`World`] helpers for
//! spawning, and the data carriers defined below.

mod events;
pub mod test_support;

use std::{
    cmp::Ordering,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering as AtomicOrdering},
    },
    time::{Duration, Instant},
};

use async_trait::async_trait;
use core_foundation::{
    array::CFArray,
    base::{CFType, TCFType},
    dictionary::CFDictionary,
    number::CFNumber,
    string::CFString,
};
use core_graphics::{
    display::CGDisplay,
    geometry::{CGPoint, CGRect, CGSize},
    window::{
        copy_window_info, kCGNullWindowID, kCGWindowBounds, kCGWindowLayer,
        kCGWindowListExcludeDesktopElements, kCGWindowListOptionOnScreenOnly, kCGWindowName,
        kCGWindowNumber, kCGWindowOwnerName, kCGWindowOwnerPID,
    },
};
pub use events::EventCursor;
use events::{DEFAULT_EVENT_CAPACITY, EventHub as InternalHub};
pub use hotki_protocol::{DisplayFrame, DisplaysSnapshot, FocusSnapshot};
use parking_lot::RwLock;
pub use permissions::{PermissionState, PermissionsStatus as Capabilities};
use permissions::{accessibility_ok, input_monitoring_ok, screen_recording_ok};
use serde::{Deserialize, Serialize};
use tokio::time::{self, Instant as TokioInstant};

/// Unique key for a window.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct WindowKey {
    /// Process identifier that owns the window.
    pub pid: i32,
    /// Window identifier (opaque, best-effort).
    pub id: u32,
}

/// Snapshot of a single window. This is intentionally minimal: app/title/pid/id
/// plus focus and display linkage.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorldWindow {
    /// Human-readable application name.
    pub app: String,
    /// Window title (best-effort; may be empty when unavailable).
    pub title: String,
    /// Owning process id.
    pub pid: i32,
    /// Opaque window identifier.
    pub id: u32,
    /// Identifier of the display containing the window, if known.
    pub display_id: Option<u32>,
    /// True if this window is considered focused.
    pub focused: bool,
}

impl WorldWindow {
    /// Identifier pairing pid and id.
    #[must_use]
    pub fn world_id(&self) -> WindowKey {
        WindowKey {
            pid: self.pid,
            id: self.id,
        }
    }
}

/// Convert a world window snapshot into the shared focus snapshot type.
#[must_use]
pub fn focus_snapshot(window: &WorldWindow) -> FocusSnapshot {
    FocusSnapshot {
        app: window.app.clone(),
        title: window.title.clone(),
        pid: window.pid,
        display_id: window.display_id,
    }
}

/// Resolve a focused snapshot from a focus change, falling back to the current world state.
pub async fn focus_snapshot_for_change(
    world: &dyn WorldView,
    change: &FocusChange,
) -> Option<FocusSnapshot> {
    if let Some(focus) = change.focus.clone() {
        return Some(focus);
    }
    let key = change.key?;
    snapshot_for_key(world, key).await
}

/// Subscribe to world events together with the current focused snapshot, if any.
pub async fn subscribe_with_snapshot(
    world: &dyn WorldView,
) -> (EventCursor, Option<FocusSnapshot>) {
    let cursor = world.subscribe();
    let focus = focused_snapshot(world).await;
    (cursor, focus)
}

/// Resolve a specific window key into the shared focus snapshot type.
pub async fn snapshot_for_key(world: &dyn WorldView, key: WindowKey) -> Option<FocusSnapshot> {
    world
        .snapshot()
        .await
        .into_iter()
        .find(|window| window.world_id() == key)
        .map(|window| focus_snapshot(&window))
}

/// Resolve the currently focused snapshot from the world state, if any.
pub async fn focused_snapshot(world: &dyn WorldView) -> Option<FocusSnapshot> {
    let key = world.focused().await?;
    snapshot_for_key(world, key).await
}

/// Context describing the current focus selection accompanying focus events.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FocusChange {
    /// Window key for the focused window, when available.
    pub key: Option<WindowKey>,
    /// Shared focused window snapshot, if available.
    pub focus: Option<FocusSnapshot>,
}

/// World events stream payloads.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorldEvent {
    /// The focused window changed, including best-effort context.
    FocusChanged(FocusChange),
    /// Display geometry snapshot changed.
    DisplaysChanged,
}

/// Diagnostic snapshot of world internals.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorldStatus {
    /// Number of windows currently tracked.
    pub windows_count: usize,
    /// Key of the currently focused window, if any.
    pub focused: Option<WindowKey>,
    /// Last polling duration in milliseconds.
    pub last_tick_ms: u64,
    /// Current polling interval in milliseconds.
    pub current_poll_ms: u64,
    /// Capability snapshot.
    pub capabilities: Capabilities,
}

/// Configuration for the world service.
#[derive(Clone, Debug)]
pub struct WorldCfg {
    /// Minimum poll interval in milliseconds.
    pub poll_ms_min: u64,
    /// Maximum poll interval in milliseconds.
    pub poll_ms_max: u64,
}

impl Default for WorldCfg {
    fn default() -> Self {
        Self {
            poll_ms_min: 200,
            poll_ms_max: 500,
        }
    }
}

/// Read-only interface exposed by the world service.
#[async_trait]
pub trait WorldView: Send + Sync {
    /// Subscribe to live [`WorldEvent`] updates.
    fn subscribe(&self) -> EventCursor;

    /// Await the next event for the given cursor until the deadline expires.
    async fn next_event_until(
        &self,
        cursor: &mut EventCursor,
        deadline: TokioInstant,
    ) -> Option<WorldEvent>;

    /// Retrieve the latest world snapshot.
    async fn snapshot(&self) -> Vec<WorldWindow>;

    /// Retrieve the currently focused window key, if any.
    async fn focused(&self) -> Option<WindowKey>;

    /// Fetch current capability and permission information.
    async fn capabilities(&self) -> Capabilities;

    /// Fetch comprehensive world status diagnostics.
    async fn status(&self) -> WorldStatus;

    /// Retrieve the tracked display geometry snapshot.
    async fn displays(&self) -> DisplaysSnapshot;

    /// Hint that external state likely changed and should be refreshed quickly.
    fn hint_refresh(&self);
}

/// Lightweight world implementation backed by periodic polling of focus + displays.
struct PollingWorld {
    core: Arc<WorldCore>,
    poll_tuner: Arc<PollTuner>,
}

struct WorldCore {
    state: Arc<WorldState>,
    hub: Arc<InternalHub>,
}

impl WorldCore {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Arc::new(WorldState::default()),
            hub: Arc::new(InternalHub::new(DEFAULT_EVENT_CAPACITY)),
        })
    }
}

#[derive(Default)]
struct WorldState {
    snapshot: RwLock<Vec<WorldWindow>>,
    focused: RwLock<Option<WindowKey>>,
    displays: RwLock<DisplaysSnapshot>,
    capabilities: RwLock<Capabilities>,
    status: RwLock<WorldStatus>,
}

impl WorldState {
    async fn snapshot(&self) -> Vec<WorldWindow> {
        self.snapshot.read().clone()
    }

    async fn focused(&self) -> Option<WindowKey> {
        *self.focused.read()
    }

    async fn capabilities(&self) -> Capabilities {
        *self.capabilities.read()
    }

    async fn status(&self) -> WorldStatus {
        self.status.read().clone()
    }

    async fn displays(&self) -> DisplaysSnapshot {
        self.displays.read().clone()
    }
}

/// Simple backoff controller for polling cadence.
struct PollTuner {
    min_ms: u64,
    max_ms: u64,
    next_ms: AtomicU64,
}

impl PollTuner {
    fn new(min_ms: u64, max_ms: u64) -> Self {
        let clamped_min = min_ms.max(50);
        Self {
            min_ms: clamped_min,
            max_ms,
            next_ms: AtomicU64::new(clamped_min),
        }
    }

    /// Compute the next interval, applying a gentle backoff up to max_ms.
    fn next_interval(&self, last_ms: u64) -> u64 {
        let proposed = last_ms.saturating_add(10).min(self.max_ms);
        self.next_ms.store(proposed, AtomicOrdering::SeqCst);
        proposed
    }

    /// Reset the cadence to the minimum to react quickly to external changes.
    fn reset(&self) {
        self.next_ms.store(self.min_ms, AtomicOrdering::SeqCst);
    }
}

impl PollingWorld {
    fn spawn(cfg: WorldCfg) -> Arc<Self> {
        let poll_tuner = Arc::new(PollTuner::new(cfg.poll_ms_min, cfg.poll_ms_max));
        let core = WorldCore::new();
        let world = Arc::new(Self {
            core,
            poll_tuner: poll_tuner.clone(),
        });

        tokio::spawn(Self::run_poll_loop(world.clone(), cfg, poll_tuner));
        world
    }

    async fn run_poll_loop(self: Arc<Self>, cfg: WorldCfg, poll_tuner: Arc<PollTuner>) {
        let mut interval_ms = cfg.poll_ms_min.max(50);
        loop {
            let start = Instant::now();
            self.poll_once().await;
            let elapsed = start.elapsed().as_millis() as u64;
            {
                let mut st = self.core.state.status.write();
                st.last_tick_ms = elapsed;
                st.current_poll_ms = interval_ms;
            }
            interval_ms = poll_tuner.next_interval(interval_ms);
            time::sleep(Duration::from_millis(interval_ms)).await;
        }
    }

    async fn poll_once(&self) {
        let platform = capture_platform_snapshot();
        let displays = platform.displays.clone();
        {
            let mut disp_guard = self.core.state.displays.write();
            if *disp_guard != displays {
                *disp_guard = displays.clone();
                self.core.hub.publish(WorldEvent::DisplaysChanged);
            }
        }

        let mut snapshot_guard = self.core.state.snapshot.write();
        let mut focus_guard = self.core.state.focused.write();
        let mut caps_guard = self.core.state.capabilities.write();

        let new_snapshot: Vec<WorldWindow> = platform
            .windows
            .iter()
            .map(|w| WorldWindow {
                app: w.app.clone(),
                title: w.title.clone(),
                pid: w.pid,
                id: w.id,
                display_id: w.display_id,
                focused: platform
                    .focused
                    .as_ref()
                    .map(|fw| fw.pid == w.pid && fw.id == w.id)
                    .unwrap_or(false),
            })
            .collect();

        let focused_key = platform.focused.as_ref().map(|w| WindowKey {
            pid: w.pid,
            id: w.id,
        });

        // Focus change detection
        if *focus_guard != focused_key {
            *focus_guard = focused_key;
            self.core.hub.publish(WorldEvent::FocusChanged(FocusChange {
                key: focused_key,
                focus: platform.focused.as_ref().map(|window| FocusSnapshot {
                    app: window.app.clone(),
                    title: window.title.clone(),
                    pid: window.pid,
                    display_id: window.display_id,
                }),
            }));
        }

        // Snapshot changes are wholesale for now.
        if *snapshot_guard != new_snapshot {
            *snapshot_guard = new_snapshot;
        }

        {
            let mut status = self.core.state.status.write();
            status.windows_count = snapshot_guard.len();
            status.focused = focused_key;
            status.capabilities = platform.capabilities;
        }

        *caps_guard = platform.capabilities;
    }
}

/// Implement `WorldView` for a type with `core: Arc<WorldCore>`.
/// The `hint_refresh` implementation must be provided separately as it differs between types.
macro_rules! impl_world_view_common {
    ($ty:ty) => {
        #[async_trait]
        impl WorldView for $ty {
            fn subscribe(&self) -> EventCursor {
                self.core.hub.subscribe()
            }

            async fn next_event_until(
                &self,
                cursor: &mut EventCursor,
                deadline: TokioInstant,
            ) -> Option<WorldEvent> {
                self.core.hub.next_event_until(cursor, deadline).await
            }

            async fn snapshot(&self) -> Vec<WorldWindow> {
                self.core.state.snapshot().await
            }

            async fn focused(&self) -> Option<WindowKey> {
                self.core.state.focused().await
            }

            async fn capabilities(&self) -> Capabilities {
                self.core.state.capabilities().await
            }

            async fn status(&self) -> WorldStatus {
                self.core.state.status().await
            }

            async fn displays(&self) -> DisplaysSnapshot {
                self.core.state.displays().await
            }

            fn hint_refresh(&self) {
                self.hint_refresh_impl()
            }
        }
    };
}

impl PollingWorld {
    fn hint_refresh_impl(&self) {
        self.poll_tuner.reset();
    }
}

impl_world_view_common!(PollingWorld);

#[derive(Clone, Debug, Default)]
struct PlatformWindow {
    app: String,
    title: String,
    pid: i32,
    id: u32,
    display_id: Option<u32>,
}

#[derive(Clone, Debug, Default)]
struct PlatformSnapshot {
    windows: Vec<PlatformWindow>,
    focused: Option<PlatformWindow>,
    displays: DisplaysSnapshot,
    capabilities: Capabilities,
}

fn capture_platform_snapshot() -> PlatformSnapshot {
    let capabilities = Capabilities {
        accessibility: accessibility_ok().into(),
        input_monitoring: input_monitoring_ok().into(),
        screen_recording: screen_recording_ok().into(),
    };

    let mut displays = gather_displays();
    let focused = active_window(&displays.displays);
    if let Some(ref fw) = focused
        && let Some(active_id) = fw.display_id
    {
        displays.active = displays
            .displays
            .iter()
            .find(|d| d.id == active_id)
            .copied()
            .or(displays.active);
    }
    if displays.active.is_none() {
        displays.active = displays.displays.first().copied();
    }
    let mut windows = Vec::new();
    if let Some(fw) = focused.clone() {
        windows.push(fw);
    }

    PlatformSnapshot {
        windows,
        focused,
        displays,
        capabilities,
    }
}

fn gather_displays() -> DisplaysSnapshot {
    let mut frames = Vec::new();
    let mut global_top = 0.0_f32;
    let main = CGDisplay::main();
    let main_bounds: CGRect = main.bounds();
    let mut active = None;

    if let Ok(active_ids) = CGDisplay::active_displays() {
        for id in active_ids {
            let display = CGDisplay::new(id);
            let bounds: CGRect = display.bounds();
            let frame = DisplayFrame {
                id: display.id,
                x: bounds.origin.x as f32,
                y: bounds.origin.y as f32,
                width: bounds.size.width as f32,
                height: bounds.size.height as f32,
            };
            if display.id == main.id {
                active = Some(frame);
            }
            global_top = global_top.max(frame.top());
            frames.push(frame);
        }
    }

    if active.is_none() {
        let fallback = DisplayFrame {
            id: main.id,
            x: main_bounds.origin.x as f32,
            y: main_bounds.origin.y as f32,
            width: main_bounds.size.width as f32,
            height: main_bounds.size.height as f32,
        };
        global_top = global_top.max(fallback.top());
        active = Some(fallback);
    }

    DisplaysSnapshot {
        global_top,
        active,
        displays: frames,
    }
}

fn active_window(displays: &[DisplayFrame]) -> Option<PlatformWindow> {
    let options = kCGWindowListOptionOnScreenOnly | kCGWindowListExcludeDesktopElements;
    let arr: CFArray = copy_window_info(options, kCGNullWindowID)?;
    let key_layer = unsafe { CFString::wrap_under_get_rule(kCGWindowLayer) };
    let key_owner_pid = unsafe { CFString::wrap_under_get_rule(kCGWindowOwnerPID) };
    let key_owner_name = unsafe { CFString::wrap_under_get_rule(kCGWindowOwnerName) };
    let key_name = unsafe { CFString::wrap_under_get_rule(kCGWindowName) };
    let key_number = unsafe { CFString::wrap_under_get_rule(kCGWindowNumber) };
    let key_bounds = unsafe { CFString::wrap_under_get_rule(kCGWindowBounds) };

    for raw in arr.iter() {
        let dict_ptr = *raw;
        let dict: CFDictionary<CFString, CFType> =
            unsafe { CFDictionary::wrap_under_get_rule(dict_ptr as _) };
        // Skip non-layer-0 windows (menus, overlays).
        let layer = dict_value_i32(&dict, &key_layer).unwrap_or(1);
        if layer != 0 {
            continue;
        }
        let Some(pid) = dict_value_i32(&dict, &key_owner_pid) else {
            continue;
        };
        let id = dict_value_u32(&dict, &key_number).unwrap_or(0);
        let app = dict_value_string(&dict, &key_owner_name).unwrap_or_default();
        let title = dict_value_string(&dict, &key_name).unwrap_or_default();
        let display_id = dict_value_rect(&dict, &key_bounds)
            .as_ref()
            .and_then(|rect| display_for_rect(rect, displays));
        return Some(PlatformWindow {
            app,
            title,
            pid,
            id,
            display_id,
        });
    }

    None
}

fn dict_value_string(dict: &CFDictionary<CFString, CFType>, key: &CFString) -> Option<String> {
    dict.find(key)
        .and_then(|v| v.downcast::<CFString>())
        .map(|s| s.to_string())
}

fn dict_value_i32(dict: &CFDictionary<CFString, CFType>, key: &CFString) -> Option<i32> {
    dict.find(key)
        .and_then(|v| v.downcast::<CFNumber>())
        .and_then(|n: CFNumber| n.to_i64())
        .map(|n| n as i32)
}

fn dict_value_u32(dict: &CFDictionary<CFString, CFType>, key: &CFString) -> Option<u32> {
    dict_value_i32(dict, key).map(|v| v as u32)
}

fn dict_value_rect(dict: &CFDictionary<CFString, CFType>, key: &CFString) -> Option<CGRect> {
    let bounds_dict: CFDictionary<CFString, CFType> =
        unsafe { CFDictionary::wrap_under_get_rule(dict.find(key)?.as_CFTypeRef() as _) };
    let x = dict_value_f32(&bounds_dict, "X")?;
    let y = dict_value_f32(&bounds_dict, "Y")?;
    let width = dict_value_f32(&bounds_dict, "Width")?;
    let height = dict_value_f32(&bounds_dict, "Height")?;
    let origin = CGPoint::new(x as f64, y as f64);
    let size = CGSize::new(width as f64, height as f64);
    Some(CGRect::new(&origin, &size))
}

fn dict_value_f32(dict: &CFDictionary<CFString, CFType>, name: &'static str) -> Option<f32> {
    let key = CFString::from_static_string(name);
    dict.find(&key)
        .and_then(|v| v.downcast::<CFNumber>())
        .and_then(|n: CFNumber| n.to_f64())
        .map(|v| v as f32)
}

fn display_for_rect(bounds: &CGRect, displays: &[DisplayFrame]) -> Option<u32> {
    if displays.is_empty() {
        return None;
    }

    let center_x = (bounds.origin.x + bounds.size.width * 0.5) as f32;
    let center_y = (bounds.origin.y + bounds.size.height * 0.5) as f32;

    if let Some(display) = displays
        .iter()
        .find(|d| point_in_display(d, center_x, center_y))
    {
        return Some(display.id);
    }

    displays
        .iter()
        .map(|d| (d.id, overlap_area(bounds, d)))
        .filter(|(_, area)| *area > 0.0)
        .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(Ordering::Equal))
        .map(|(id, _)| id)
}

fn point_in_display(display: &DisplayFrame, x: f32, y: f32) -> bool {
    x >= display.x
        && x <= display.x + display.width
        && y >= display.y
        && y <= display.y + display.height
}

fn overlap_area(bounds: &CGRect, display: &DisplayFrame) -> f32 {
    let left = bounds.origin.x.max(display.x as f64) as f32;
    let right =
        (bounds.origin.x + bounds.size.width).min((display.x + display.width) as f64) as f32;
    let bottom = bounds.origin.y.max(display.y as f64) as f32;
    let top =
        (bounds.origin.y + bounds.size.height).min((display.y + display.height) as f64) as f32;

    let width = (right - left).max(0.0);
    let height = (top - bottom).max(0.0);
    width * height
}

/// Public helpers to spawn world views.
pub struct World;

impl World {
    /// Spawn the default polling world view.
    pub fn spawn_default_view(cfg: WorldCfg) -> Arc<dyn WorldView> {
        PollingWorld::spawn(cfg)
    }
}

/// Simple in-memory world used for tests and fixtures.
pub struct TestWorld {
    core: Arc<WorldCore>,
}

impl TestWorld {
    /// Create an empty test world.
    pub fn new() -> Self {
        Self {
            core: WorldCore::new(),
        }
    }

    /// Replace the snapshot and focused key atomically.
    pub fn set_snapshot(&self, snapshot: Vec<WorldWindow>, focused: Option<WindowKey>) {
        let mut snap_guard = self.core.state.snapshot.write();
        let mut foc_guard = self.core.state.focused.write();
        let prev_focus = *foc_guard;
        *snap_guard = snapshot.clone();
        *foc_guard = focused;

        if prev_focus != focused {
            let change = if let Some(key) = focused {
                snapshot
                    .iter()
                    .find(|w| w.world_id() == key)
                    .map(|w| FocusChange {
                        key: Some(key),
                        focus: Some(focus_snapshot(w)),
                    })
                    .unwrap_or(FocusChange {
                        key: Some(key),
                        focus: None,
                    })
            } else {
                FocusChange::default()
            };
            self.core.hub.publish(WorldEvent::FocusChanged(change));
        }
    }

    /// Push a synthetic event onto the stream.
    pub fn push_event(&self, event: WorldEvent) {
        self.core.hub.publish(event);
    }

    /// Replace the tracked display snapshot.
    pub fn set_displays(&self, displays: DisplaysSnapshot) {
        *self.core.state.displays.write() = displays;
        self.core.hub.publish(WorldEvent::DisplaysChanged);
    }
}

impl Default for TestWorld {
    fn default() -> Self {
        Self::new()
    }
}

impl TestWorld {
    fn hint_refresh_impl(&self) {
        // No-op for test world
    }
}

impl_world_view_common!(TestWorld);
