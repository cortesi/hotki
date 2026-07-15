use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::RwLock;
use tokio::time::Instant as TokioInstant;

use crate::{
    Capabilities, DisplayFrame, DisplaysSnapshot, EventCursor, FocusChange, FocusSnapshot,
    WindowKey, WorldEvent, WorldStatus, WorldView, WorldWindow,
    events::{DEFAULT_EVENT_CAPACITY, EventHub as InternalHub},
};

pub(crate) struct WorldCore {
    pub(crate) state: Arc<WorldState>,
    pub(crate) hub: Arc<InternalHub>,
}

impl WorldCore {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Arc::new(WorldState::default()),
            hub: Arc::new(InternalHub::new(DEFAULT_EVENT_CAPACITY)),
        })
    }
}

#[derive(Default, Clone)]
struct WorldStateData {
    snapshot: Vec<WorldWindow>,
    focused: Option<WindowKey>,
    focus: Option<FocusSnapshot>,
    displays: DisplaysSnapshot,
    capabilities: Capabilities,
    status: WorldStatus,
}

#[derive(Default)]
pub(crate) struct WorldState {
    data: RwLock<WorldStateData>,
}

impl WorldState {
    pub(crate) fn snapshot(&self) -> Vec<WorldWindow> {
        self.data.read().snapshot.clone()
    }

    pub(crate) fn focused(&self) -> Option<WindowKey> {
        self.data.read().focused
    }

    pub(crate) fn focus_snapshot(&self) -> Option<FocusSnapshot> {
        self.data.read().focus.clone()
    }

    pub(crate) fn capabilities(&self) -> Capabilities {
        self.data.read().capabilities
    }

    pub(crate) fn status(&self) -> WorldStatus {
        self.data.read().status.clone()
    }

    pub(crate) fn displays(&self) -> DisplaysSnapshot {
        self.data.read().displays.clone()
    }

    pub(crate) fn apply_poll_update(
        &self,
        update: WorldPollUpdate,
        last_tick_ms: u64,
        current_poll_ms: u64,
    ) -> WorldPollChanges {
        let mut data = self.data.write();

        let displays_changed = data.displays != update.displays;
        if displays_changed {
            data.displays = update.displays;
        }

        let focus_changed = if data.focused != update.focused || data.focus != update.focus {
            Some(FocusChange {
                key: update.focused,
                focus: update.focus.clone(),
            })
        } else {
            None
        };
        data.focused = update.focused;
        data.focus = update.focus;

        if data.snapshot != update.snapshot {
            data.snapshot = update.snapshot;
        }

        data.capabilities = update.capabilities;
        data.status.windows_count = data.snapshot.len();
        data.status.focused = data.focused;
        data.status.last_tick_ms = last_tick_ms;
        data.status.current_poll_ms = current_poll_ms;
        data.status.capabilities = update.capabilities;

        WorldPollChanges {
            displays_changed,
            focus_changed,
        }
    }

    #[cfg(any(test, feature = "test-utils"))]
    pub(crate) fn set_snapshot(
        &self,
        snapshot: Vec<WorldWindow>,
        focused: Option<WindowKey>,
    ) -> Option<FocusChange> {
        let change = focus_change_for_snapshot(&snapshot, focused);
        let changed = {
            let mut data = self.data.write();
            let changed = data.focused != change.key || data.focus != change.focus;
            data.snapshot = snapshot;
            data.focused = focused;
            data.focus = change.focus.clone();
            data.status.windows_count = data.snapshot.len();
            data.status.focused = focused;
            changed
        };

        changed.then_some(change)
    }

    #[cfg(any(test, feature = "test-utils"))]
    pub(crate) fn set_displays(&self, displays: DisplaysSnapshot) {
        self.data.write().displays = displays;
    }
}

pub(crate) struct WorldPollUpdate {
    pub(crate) snapshot: Vec<WorldWindow>,
    pub(crate) focused: Option<WindowKey>,
    pub(crate) focus: Option<FocusSnapshot>,
    pub(crate) displays: DisplaysSnapshot,
    pub(crate) capabilities: Capabilities,
}

pub(crate) struct WorldPollChanges {
    pub(crate) displays_changed: bool,
    pub(crate) focus_changed: Option<FocusChange>,
}

impl WorldPollChanges {
    pub(crate) fn publish(self, hub: &InternalHub) {
        if self.displays_changed {
            hub.publish(WorldEvent::DisplaysChanged);
        }
        if let Some(change) = self.focus_changed {
            hub.publish(WorldEvent::FocusChanged(change));
        }
    }
}

#[async_trait]
pub(crate) trait CoreWorldView {
    fn core(&self) -> &Arc<WorldCore>;

    fn resolve_application_impl(&self, app_name: &str) -> crate::ApplicationResolution;

    async fn refresh_impl(&self);
}

#[async_trait]
impl<T> WorldView for T
where
    T: CoreWorldView + Send + Sync,
{
    fn subscribe(&self) -> EventCursor {
        self.core().hub.subscribe()
    }

    async fn next_event_until(
        &self,
        cursor: &mut EventCursor,
        deadline: TokioInstant,
    ) -> Option<WorldEvent> {
        self.core().hub.next_event_until(cursor, deadline).await
    }

    async fn snapshot(&self) -> Vec<WorldWindow> {
        self.core().state.snapshot()
    }

    async fn focused(&self) -> Option<WindowKey> {
        self.core().state.focused()
    }

    async fn focus_snapshot(&self) -> Option<FocusSnapshot> {
        self.core().state.focus_snapshot()
    }

    async fn capabilities(&self) -> Capabilities {
        self.core().state.capabilities()
    }

    async fn status(&self) -> WorldStatus {
        self.core().state.status()
    }

    async fn resolve_application(&self, app_name: &str) -> crate::ApplicationResolution {
        self.resolve_application_impl(app_name)
    }

    async fn displays(&self) -> DisplaysSnapshot {
        self.core().state.displays()
    }

    async fn refresh(&self) {
        self.refresh_impl().await;
    }
}

#[cfg(any(test, feature = "test-utils"))]
fn focus_change_for_snapshot(snapshot: &[WorldWindow], focused: Option<WindowKey>) -> FocusChange {
    let Some(key) = focused else {
        return FocusChange::default();
    };

    snapshot
        .iter()
        .find(|window| window.world_id() == key)
        .map(|window| FocusChange {
            key: Some(key),
            focus: Some(crate::focus_snapshot(window)),
        })
        .unwrap_or(FocusChange {
            key: Some(key),
            focus: None,
        })
}

pub(crate) fn display_frame(id: u32, x: f32, y: f32, width: f32, height: f32) -> DisplayFrame {
    DisplayFrame {
        id,
        x,
        y,
        width,
        height,
    }
}
