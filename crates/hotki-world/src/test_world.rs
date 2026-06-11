use std::sync::Arc;

use crate::{
    DisplaysSnapshot, WorldEvent, WorldWindow,
    state::{CoreWorldView, WorldCore},
    types::WindowKey,
};

/// Simple in-memory world used for tests and fixtures.
pub struct TestWorld {
    core: Arc<WorldCore>,
}

impl TestWorld {
    /// Create an empty test world.
    #[must_use]
    pub fn new() -> Self {
        Self {
            core: WorldCore::new(),
        }
    }

    /// Replace the snapshot and focused key atomically.
    pub fn set_snapshot(&self, snapshot: Vec<WorldWindow>, focused: Option<WindowKey>) {
        if let Some(change) = self.core.state.set_snapshot(snapshot, focused) {
            self.core.hub.publish(WorldEvent::FocusChanged(change));
        }
    }

    /// Push a synthetic event onto the stream.
    pub fn push_event(&self, event: WorldEvent) {
        self.core.hub.publish(event);
    }

    /// Replace the tracked display snapshot.
    pub fn set_displays(&self, displays: DisplaysSnapshot) {
        self.core.state.set_displays(displays);
        self.core.hub.publish(WorldEvent::DisplaysChanged);
    }
}

impl Default for TestWorld {
    fn default() -> Self {
        Self::new()
    }
}

impl CoreWorldView for TestWorld {
    fn core(&self) -> &Arc<WorldCore> {
        &self.core
    }

    fn hint_refresh_impl(&self) {}
}
