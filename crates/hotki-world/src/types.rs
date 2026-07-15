use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::time::Instant as TokioInstant;

use crate::{Capabilities, DisplaysSnapshot, EventCursor, FocusSnapshot};

/// Result of resolving one exact running application name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApplicationResolution {
    /// Exactly one running process has the requested localized name.
    Found(i32),
    /// No running process has the requested localized name.
    NotRunning,
    /// Multiple distinct running processes have the requested localized name.
    Ambiguous(usize),
}

/// Minimal running-application record used by platform and deterministic worlds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RunningApplication {
    pub(crate) name: Option<String>,
    pub(crate) pid: i32,
    pub(crate) terminated: bool,
}

/// Resolve an exact localized name from a running-application snapshot.
pub(crate) fn resolve_application(
    applications: &[RunningApplication],
    app_name: &str,
) -> ApplicationResolution {
    let mut pids: Vec<_> = applications
        .iter()
        .filter(|application| {
            !application.terminated
                && application.pid > 0
                && application.name.as_deref() == Some(app_name)
        })
        .map(|application| application.pid)
        .collect();
    pids.sort_unstable();
    pids.dedup();

    match pids.as_slice() {
        [] => ApplicationResolution::NotRunning,
        [pid] => ApplicationResolution::Found(*pid),
        pids => ApplicationResolution::Ambiguous(pids.len()),
    }
}

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
        id: window.id,
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
    world.focus_snapshot().await
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

    /// Retrieve the semantic snapshot of the currently focused window, if any.
    async fn focus_snapshot(&self) -> Option<FocusSnapshot>;

    /// Fetch current capability and permission information.
    async fn capabilities(&self) -> Capabilities;

    /// Fetch comprehensive world status diagnostics.
    async fn status(&self) -> WorldStatus;

    /// Resolve one exact AppKit localized name to a running process.
    async fn resolve_application(&self, app_name: &str) -> ApplicationResolution;

    /// Retrieve the tracked display geometry snapshot.
    async fn displays(&self) -> DisplaysSnapshot;

    /// Wait until a refresh begun after this call has updated the world state.
    async fn refresh(&self);
}
