use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::broadcast;

use crate::{
    Capabilities, CommandError, CommandReceipt, FullscreenIntent, HideIntent, MoveIntent,
    PlaceIntent, RaiseIntent, WindowKey, WorldEvent, WorldHandle, WorldStatus, WorldWindow,
};

/// Unified view over window state snapshots and focus context.
#[async_trait]
pub trait WorldView: Send + Sync {
    /// Subscribe to live [`WorldEvent`] updates.
    fn subscribe(&self) -> broadcast::Receiver<WorldEvent>;

    /// Subscribe and obtain an initial snapshot plus focused key.
    async fn subscribe_with_snapshot(
        &self,
    ) -> (
        broadcast::Receiver<WorldEvent>,
        Vec<WorldWindow>,
        Option<WindowKey>,
    );

    /// Subscribe and obtain a derived focus context `(app, title, pid)` if any.
    async fn subscribe_with_context(
        &self,
    ) -> (
        broadcast::Receiver<WorldEvent>,
        Option<(String, String, i32)>,
    );

    /// Retrieve the latest world snapshot.
    async fn snapshot(&self) -> Vec<WorldWindow>;

    /// Resolve a [`WindowKey`] to its current [`WorldWindow`], if present.
    async fn get(&self, key: WindowKey) -> Option<WorldWindow>;

    /// Retrieve the currently focused window key, if any.
    async fn focused(&self) -> Option<WindowKey>;

    /// Retrieve the currently focused window with full metadata, if any.
    async fn focused_window(&self) -> Option<WorldWindow>;

    /// Retrieve a lightweight `(app, title, pid)` tuple for the focused window, if any.
    async fn focused_context(&self) -> Option<(String, String, i32)>;

    /// Resolve a `WindowKey` into its context tuple if the window is still present.
    async fn context_for_key(&self, key: WindowKey) -> Option<(String, String, i32)>;

    /// Fetch current capability and permission information.
    async fn capabilities(&self) -> Capabilities;

    /// Fetch comprehensive world status diagnostics.
    async fn status(&self) -> WorldStatus;

    /// Hint that external state likely changed and should be refreshed quickly.
    fn hint_refresh(&self);

    /// Fetch a complete snapshot of current windows.
    async fn list_windows(&self) -> Vec<WorldWindow> {
        self.snapshot().await
    }

    /// Resolve the frontmost window, preferring focus information.
    async fn frontmost_window(&self) -> Option<WorldWindow> {
        if let Some(focused) = self.focused_window().await {
            return Some(focused);
        }
        self.snapshot().await.into_iter().min_by_key(|w| w.z)
    }

    /// Resolve a [`WindowKey`] using the latest snapshot.
    async fn resolve_key(&self, key: WindowKey) -> Option<WorldWindow> {
        self.snapshot()
            .await
            .into_iter()
            .find(|w| w.pid == key.pid && w.id == key.id)
    }

    /// Resolve a window by process identifier and title, if still present.
    async fn window_by_pid_title(&self, pid: i32, title: &str) -> Option<WorldWindow> {
        self.snapshot()
            .await
            .into_iter()
            .find(|w| w.pid == pid && w.title == title)
    }

    /// Queue a grid placement command.
    async fn request_place_grid(&self, intent: PlaceIntent)
    -> Result<CommandReceipt, CommandError>;

    /// Queue a relative placement move command.
    async fn request_place_move_grid(
        &self,
        intent: MoveIntent,
    ) -> Result<CommandReceipt, CommandError>;

    /// Queue a hide/show command for the active application.
    async fn request_hide(&self, intent: HideIntent) -> Result<CommandReceipt, CommandError>;

    /// Queue a fullscreen command for the active application.
    async fn request_fullscreen(
        &self,
        intent: FullscreenIntent,
    ) -> Result<CommandReceipt, CommandError>;

    /// Queue a raise command using optional regex filters.
    async fn request_raise(&self, intent: RaiseIntent) -> Result<CommandReceipt, CommandError>;
}

#[async_trait]
impl WorldView for WorldHandle {
    fn subscribe(&self) -> broadcast::Receiver<WorldEvent> {
        WorldHandle::subscribe(self)
    }

    async fn subscribe_with_snapshot(
        &self,
    ) -> (
        broadcast::Receiver<WorldEvent>,
        Vec<WorldWindow>,
        Option<WindowKey>,
    ) {
        WorldHandle::subscribe_with_snapshot(self).await
    }

    async fn subscribe_with_context(
        &self,
    ) -> (
        broadcast::Receiver<WorldEvent>,
        Option<(String, String, i32)>,
    ) {
        WorldHandle::subscribe_with_context(self).await
    }

    async fn snapshot(&self) -> Vec<WorldWindow> {
        WorldHandle::snapshot(self).await
    }

    async fn get(&self, key: WindowKey) -> Option<WorldWindow> {
        WorldHandle::get(self, key).await
    }

    async fn focused(&self) -> Option<WindowKey> {
        WorldHandle::focused(self).await
    }

    async fn focused_window(&self) -> Option<WorldWindow> {
        WorldHandle::focused_window(self).await
    }

    async fn focused_context(&self) -> Option<(String, String, i32)> {
        WorldHandle::focused_context(self).await
    }

    async fn context_for_key(&self, key: WindowKey) -> Option<(String, String, i32)> {
        WorldHandle::context_for_key(self, key).await
    }

    async fn capabilities(&self) -> Capabilities {
        WorldHandle::capabilities(self).await
    }

    async fn status(&self) -> WorldStatus {
        WorldHandle::status(self).await
    }

    fn hint_refresh(&self) {
        WorldHandle::hint_refresh(self);
    }

    async fn resolve_key(&self, key: WindowKey) -> Option<WorldWindow> {
        WorldHandle::get(self, key).await
    }

    async fn request_place_grid(
        &self,
        intent: PlaceIntent,
    ) -> Result<CommandReceipt, CommandError> {
        WorldHandle::request_place_grid(self, intent).await
    }

    async fn request_place_move_grid(
        &self,
        intent: MoveIntent,
    ) -> Result<CommandReceipt, CommandError> {
        WorldHandle::request_place_move_grid(self, intent).await
    }

    async fn request_hide(&self, intent: HideIntent) -> Result<CommandReceipt, CommandError> {
        WorldHandle::request_hide(self, intent).await
    }

    async fn request_fullscreen(
        &self,
        intent: FullscreenIntent,
    ) -> Result<CommandReceipt, CommandError> {
        WorldHandle::request_fullscreen(self, intent).await
    }

    async fn request_raise(&self, intent: RaiseIntent) -> Result<CommandReceipt, CommandError> {
        WorldHandle::request_raise(self, intent).await
    }
}

impl WorldHandle {
    /// Wrap the handle in an [`Arc`] for trait-object use.
    pub fn into_view(self) -> Arc<dyn WorldView> {
        Arc::new(self)
    }
}

#[cfg(any(test, feature = "test-utils"))]
mod test_world {
    use std::sync::Arc;

    use parking_lot::RwLock;
    use tokio::sync::broadcast;

    use super::WorldView;
    use crate::{
        Capabilities, CommandError, CommandReceipt, FullscreenIntent, HideIntent, MoveIntent,
        PlaceIntent, RaiseIntent, WindowKey, WorldEvent, WorldStatus, WorldWindow,
    };

    #[derive(Default)]
    struct TestState {
        snapshot: Vec<WorldWindow>,
        focused: Option<WindowKey>,
        capabilities: Capabilities,
        status: WorldStatus,
        hint_refreshes: u64,
    }

    /// Deterministic in-memory [`WorldView`] implementation for unit and smoke tests.
    pub struct TestWorld {
        state: RwLock<TestState>,
        events: broadcast::Sender<WorldEvent>,
    }

    impl TestWorld {
        /// Create an empty test world.
        pub fn new() -> Self {
            let (events, _) = broadcast::channel(32);
            Self {
                state: RwLock::new(TestState::default()),
                events,
            }
        }

        /// Produce an [`Arc`] trait object from this test world.
        pub fn into_view(self) -> Arc<dyn WorldView> {
            Arc::new(self)
        }

        /// Replace the snapshot and focused key atomically.
        pub fn set_snapshot(&self, snapshot: Vec<WorldWindow>, focused: Option<WindowKey>) {
            let mut state = self.state.write();
            state.snapshot = snapshot;
            state.focused = focused;
        }

        /// Update the stored capability information.
        pub fn set_capabilities(&self, capabilities: Capabilities) {
            self.state.write().capabilities = capabilities;
        }

        /// Update the stored status diagnostic payload.
        pub fn set_status(&self, status: WorldStatus) {
            self.state.write().status = status;
        }

        /// Push a synthetic event onto the broadcast stream.
        pub fn push_event(&self, event: WorldEvent) {
            let _ = self.events.send(event);
        }

        /// Retrieve the number of refresh hints seen so far.
        pub fn hint_refresh_count(&self) -> u64 {
            self.state.read().hint_refreshes
        }
    }

    impl Default for TestWorld {
        fn default() -> Self {
            Self::new()
        }
    }

    #[async_trait::async_trait]
    impl WorldView for TestWorld {
        fn subscribe(&self) -> broadcast::Receiver<WorldEvent> {
            self.events.subscribe()
        }

        async fn subscribe_with_snapshot(
            &self,
        ) -> (
            broadcast::Receiver<WorldEvent>,
            Vec<WorldWindow>,
            Option<WindowKey>,
        ) {
            let rx = self.events.subscribe();
            let state = self.state.read();
            (rx, state.snapshot.clone(), state.focused)
        }

        async fn subscribe_with_context(
            &self,
        ) -> (
            broadcast::Receiver<WorldEvent>,
            Option<(String, String, i32)>,
        ) {
            let rx = self.events.subscribe();
            let ctx = self.focused_context().await;
            (rx, ctx)
        }

        async fn snapshot(&self) -> Vec<WorldWindow> {
            self.state.read().snapshot.clone()
        }

        async fn get(&self, key: WindowKey) -> Option<WorldWindow> {
            self.state
                .read()
                .snapshot
                .iter()
                .find(|w| w.pid == key.pid && w.id == key.id)
                .cloned()
        }

        async fn focused(&self) -> Option<WindowKey> {
            self.state.read().focused
        }

        async fn focused_window(&self) -> Option<WorldWindow> {
            let state = self.state.read();
            let focused = state.focused?;
            state
                .snapshot
                .iter()
                .find(|w| w.pid == focused.pid && w.id == focused.id)
                .cloned()
        }

        async fn focused_context(&self) -> Option<(String, String, i32)> {
            let state = self.state.read();
            if let Some(focused) = state.focused
                && let Some(w) = state
                    .snapshot
                    .iter()
                    .find(|w| w.pid == focused.pid && w.id == focused.id)
            {
                return Some((w.app.clone(), w.title.clone(), w.pid));
            }
            state
                .snapshot
                .iter()
                .min_by_key(|w| w.z)
                .map(|w| (w.app.clone(), w.title.clone(), w.pid))
        }

        async fn context_for_key(&self, key: WindowKey) -> Option<(String, String, i32)> {
            let state = self.state.read();
            state
                .snapshot
                .iter()
                .find(|w| w.pid == key.pid && w.id == key.id)
                .map(|w| (w.app.clone(), w.title.clone(), w.pid))
        }

        async fn capabilities(&self) -> Capabilities {
            self.state.read().capabilities.clone()
        }

        async fn status(&self) -> WorldStatus {
            self.state.read().status.clone()
        }

        fn hint_refresh(&self) {
            self.state.write().hint_refreshes += 1;
        }

        async fn request_place_grid(
            &self,
            _intent: PlaceIntent,
        ) -> Result<CommandReceipt, CommandError> {
            Err(CommandError::InvalidRequest {
                message: "TestWorld does not orchestrate placement".into(),
            })
        }

        async fn request_place_move_grid(
            &self,
            _intent: MoveIntent,
        ) -> Result<CommandReceipt, CommandError> {
            Err(CommandError::InvalidRequest {
                message: "TestWorld does not orchestrate placement".into(),
            })
        }

        async fn request_hide(&self, _intent: HideIntent) -> Result<CommandReceipt, CommandError> {
            Err(CommandError::InvalidRequest {
                message: "TestWorld does not orchestrate hide commands".into(),
            })
        }

        async fn request_fullscreen(
            &self,
            _intent: FullscreenIntent,
        ) -> Result<CommandReceipt, CommandError> {
            Err(CommandError::InvalidRequest {
                message: "TestWorld does not orchestrate fullscreen commands".into(),
            })
        }

        async fn request_raise(
            &self,
            _intent: RaiseIntent,
        ) -> Result<CommandReceipt, CommandError> {
            Err(CommandError::InvalidRequest {
                message: "TestWorld does not orchestrate raise commands".into(),
            })
        }
    }

    #[cfg(test)]
    mod tests {
        use std::time::Instant;

        use super::TestWorld;
        use crate::{WindowKey, WorldEvent, WorldView, WorldWindow};

        fn basic_window(z: u32, focused: bool) -> WorldWindow {
            WorldWindow {
                app: "App".into(),
                title: format!("W{}", z),
                pid: 42,
                id: z,
                pos: None,
                layer: 0,
                z,
                space: Some(1),
                on_active_space: true,
                is_on_screen: true,
                display_id: None,
                focused,
                ax: None,
                meta: Vec::new(),
                last_seen: Instant::now(),
                seen_seq: z as u64,
            }
        }

        #[tokio::test]
        async fn test_world_snapshot_and_focus() {
            let world = TestWorld::new();
            world.set_snapshot(
                vec![basic_window(1, true)],
                Some(WindowKey { pid: 42, id: 1 }),
            );

            let snapshot = world.snapshot().await;
            assert_eq!(snapshot.len(), 1);

            let focused = world.focused_window().await;
            assert!(focused.is_some());
            let ctx = world.focused_context().await;
            assert_eq!(ctx, Some(("App".into(), "W1".into(), 42)));
        }

        #[tokio::test]
        async fn test_world_events_and_hint() {
            let world = TestWorld::new();
            let mut rx = world.subscribe();
            world.push_event(WorldEvent::FocusChanged(None));
            assert!(rx.recv().await.is_ok());

            assert_eq!(world.hint_refresh_count(), 0);
            world.hint_refresh();
            assert_eq!(world.hint_refresh_count(), 1);
        }
    }
}

#[cfg(any(test, feature = "test-utils"))]
pub use test_world::TestWorld;
