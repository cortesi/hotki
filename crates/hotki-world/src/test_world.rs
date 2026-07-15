use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::RwLock;

use crate::{
    ApplicationResolution, DisplaysSnapshot, WorldEvent, WorldWindow,
    state::{CoreWorldView, WorldCore},
    types::{RunningApplication, WindowKey, resolve_application},
};

/// Running-application fixture for [`TestWorld`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TestApplication {
    /// Exact localized application name, or `None` when unavailable.
    pub name: Option<String>,
    /// Process identifier reported by AppKit.
    pub pid: i32,
    /// Whether AppKit reports that the process has terminated.
    pub terminated: bool,
}

/// Simple in-memory world used for tests and fixtures.
pub struct TestWorld {
    core: Arc<WorldCore>,
    applications: RwLock<Vec<RunningApplication>>,
}

impl TestWorld {
    /// Create an empty test world.
    #[must_use]
    pub fn new() -> Self {
        Self {
            core: WorldCore::new(),
            applications: RwLock::new(Vec::new()),
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

    /// Replace the deterministic running-application snapshot.
    pub fn set_running_applications(&self, applications: Vec<TestApplication>) {
        *self.applications.write() = applications
            .into_iter()
            .map(|application| RunningApplication {
                name: application.name,
                pid: application.pid,
                terminated: application.terminated,
            })
            .collect();
    }
}

impl Default for TestWorld {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl CoreWorldView for TestWorld {
    fn core(&self) -> &Arc<WorldCore> {
        &self.core
    }

    fn resolve_application_impl(&self, app_name: &str) -> ApplicationResolution {
        resolve_application(&self.applications.read(), app_name)
    }

    async fn refresh_impl(&self) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::WorldView;

    fn application(name: Option<&str>, pid: i32, terminated: bool) -> TestApplication {
        TestApplication {
            name: name.map(str::to_string),
            pid,
            terminated,
        }
    }

    #[tokio::test]
    async fn application_resolution_is_exact_deduplicated_and_window_independent() {
        let world = TestWorld::new();
        world.set_snapshot(Vec::new(), None);
        world.set_running_applications(vec![
            application(Some("YouTube Music"), 41, false),
            application(Some("YouTube Music"), 41, false),
            application(Some("YouTube Music"), 42, true),
            application(Some("YouTube Music"), 0, false),
            application(Some("youtube music"), 43, false),
            application(None, 44, false),
        ]);

        assert_eq!(
            world.resolve_application("YouTube Music").await,
            ApplicationResolution::Found(41)
        );
        assert_eq!(
            world.resolve_application("Missing").await,
            ApplicationResolution::NotRunning
        );

        world.set_running_applications(vec![
            application(Some("YouTube Music"), 41, false),
            application(Some("YouTube Music"), 45, false),
            application(Some("YouTube Music"), 45, false),
        ]);
        assert_eq!(
            world.resolve_application("YouTube Music").await,
            ApplicationResolution::Ambiguous(2)
        );
    }
}
