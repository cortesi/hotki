use std::sync::Arc;

use hotki_protocol::DisplaysSnapshot;
use hotki_world::{FocusChange, WorldView};
use tracing::{debug, trace, warn};

use super::*;

impl Engine {
    /// Access the world view for event subscriptions and snapshots.
    pub fn world(&self) -> Arc<dyn WorldView> {
        self.world.clone()
    }

    pub(crate) fn spawn_world_focus_subscription(&self) {
        let world = self.world.clone();
        let engine = self.clone_for_background();
        let cancel = self.background_cancellation_token();
        let task = tokio::spawn(async move {
            loop {
                let (mut cursor, seed) = tokio::select! {
                    () = cancel.cancelled() => return,
                    subscription = hotki_world::subscribe_with_snapshot(world.as_ref()) => {
                        subscription
                    }
                };
                if let Err(err) = engine.apply_world_focus_snapshot(seed).await {
                    warn!("World focus seed apply failed: {}", err);
                }

                let mut last_lost = cursor.lost_count;
                loop {
                    let deadline =
                        tokio::time::Instant::now() + tokio::time::Duration::from_secs(30);
                    let event = tokio::select! {
                        () = cancel.cancelled() => return,
                        event = world.next_event_until(&mut cursor, deadline) => event,
                    };
                    match event {
                        Some(event) => {
                            if cursor.lost_count > last_lost {
                                warn!(
                                    lost = cursor.lost_count - last_lost,
                                    "World focus subscription observed lost events; resubscribing"
                                );
                                break;
                            }
                            last_lost = cursor.lost_count;
                            if let hotki_world::WorldEvent::FocusChanged(change) = event {
                                engine
                                    .handle_focus_change_event(world.clone(), change)
                                    .await;
                            }
                            if let Err(err) = engine.refresh_displays_if_changed(&world).await {
                                warn!("Display refresh after world event failed: {}", err);
                            }
                        }
                        None => {
                            if cursor.is_closed() {
                                warn!("World focus subscription closed; exiting");
                                return;
                            }
                            if let Err(err) = engine.refresh_displays_if_changed(&world).await {
                                warn!("Display refresh after world timeout failed: {}", err);
                            }
                        }
                    }
                }
            }
        });
        self.register_background_task(task);
    }

    pub(crate) fn spawn_selector_notify_task(&self) {
        let engine = self.clone_for_background();
        let notify = self.selector_notify.clone();
        let cancel = self.background_cancellation_token();
        let task = tokio::spawn(async move {
            loop {
                tokio::select! {
                    () = cancel.cancelled() => return,
                    () = notify.notified() => {}
                }
                if let Err(err) = engine.on_selector_notify().await {
                    warn!("Selector notify tick failed: {}", err);
                }
            }
        });
        self.register_background_task(task);
    }

    async fn on_selector_notify(&self) -> Result<()> {
        let snapshot = {
            let mut rt = self.runtime.lock().await;
            let Some(sel) = rt.selector.as_mut() else {
                return Ok(());
            };
            if !sel.tick() {
                return Ok(());
            }
            sel.snapshot()
        };
        self.notifier
            .send_ui(hotki_protocol::MsgToUI::SelectorUpdate(snapshot))?;
        Ok(())
    }

    async fn apply_world_focus_snapshot(
        &self,
        focus: Option<hotki_protocol::FocusSnapshot>,
    ) -> Result<()> {
        let mut changed = false;
        {
            let mut guard = self.focus_ctx.lock();
            if guard.as_ref() != focus.as_ref() {
                *guard = focus.clone();
                changed = true;
            }
        }
        if !changed {
            trace!("World focus context unchanged; skipping rebind");
            return Ok(());
        }
        if let Some(ref focus) = focus {
            debug!(
                pid = focus.pid,
                app = %focus.app,
                title = %focus.title,
                "Engine: world focus context updated"
            );
        } else {
            debug!("Engine: world focus context cleared");
        }
        self.rebind_current_context().await
    }

    pub(crate) async fn refresh_world_focus(&self) -> Result<()> {
        self.world.refresh().await;
        let focus = hotki_world::focused_snapshot(self.world.as_ref()).await;
        self.apply_world_focus_snapshot(focus).await
    }

    async fn handle_focus_change_event(&self, world: Arc<dyn WorldView>, change: FocusChange) {
        let focus = hotki_world::focus_snapshot_for_change(world.as_ref(), &change).await;

        if let Some(focus) = focus {
            if let Err(err) = self.apply_world_focus_snapshot(Some(focus)).await {
                warn!("World focus update failed: {}", err);
            }
        } else if change.key.is_none() {
            if let Err(err) = self.apply_world_focus_snapshot(None).await {
                warn!("World focus clear failed: {}", err);
            }
        } else {
            warn!(key = ?change.key, "World focus context unavailable after focus change");
        }
    }

    pub(crate) async fn rebind_current_context(&self) -> Result<()> {
        let focus = self.current_focus_snapshot();
        debug!(focus = ?focus, "Rebinding with focused-window snapshot");
        self.rebind_and_refresh(&focus).await
    }

    async fn refresh_displays_if_changed(&self, world: &Arc<dyn WorldView>) -> Result<()> {
        let snapshot = world.displays().await;
        {
            let cache = self.display_snapshot.lock().await;
            if *cache == snapshot {
                return Ok(());
            }
        }

        let hud = {
            let rt = self.runtime.lock().await;
            crate::refresh::hud_state_for_ui_from_state(&rt)
        };
        self.publish_hud_with_displays(hud, snapshot).await
    }

    pub(crate) async fn publish_hud_with_displays(
        &self,
        hud: hotki_protocol::HudState,
        snapshot: DisplaysSnapshot,
    ) -> Result<()> {
        {
            let mut cache = self.display_snapshot.lock().await;
            *cache = snapshot.clone();
        }
        self.notifier.send_hud_update(hud, snapshot)?;
        Ok(())
    }

    pub(crate) fn current_focus_snapshot(&self) -> Option<hotki_protocol::FocusSnapshot> {
        self.focus_ctx.lock().clone()
    }
}

#[cfg(test)]
mod tests {
    use std::{sync::Arc, time::Duration};

    use hotki_protocol::FocusSnapshot;
    use hotki_world::{FocusChange, TestWorld, WindowKey, WorldEvent, WorldWindow};
    use tokio::sync::mpsc;

    use crate::{Engine, deps::MockHotkeyApi};

    #[tokio::test]
    async fn focus_events_apply_in_stream_order() {
        let world = Arc::new(TestWorld::new());
        world.set_snapshot(
            vec![WorldWindow {
                app: "Seed".into(),
                title: "Initial".into(),
                pid: 1,
                id: 1,
                display_id: None,
                focused: true,
            }],
            Some(WindowKey { pid: 1, id: 1 }),
        );
        let (tx, _rx) = mpsc::channel(128);
        let engine = Engine::new_with_api_and_world(
            Arc::new(MockHotkeyApi::new()),
            tx,
            false,
            world.clone(),
        );

        tokio::time::timeout(Duration::from_millis(200), async {
            loop {
                if engine
                    .current_focus_snapshot()
                    .is_some_and(|focus| focus.pid == 1)
                {
                    return;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("engine did not observe seed focus");

        world.push_event(WorldEvent::FocusChanged(FocusChange {
            key: Some(WindowKey { pid: 1, id: 1 }),
            focus: Some(FocusSnapshot {
                id: 1,
                app: "Old".into(),
                title: "First".into(),
                pid: 1,
                display_id: None,
            }),
        }));
        world.push_event(WorldEvent::FocusChanged(FocusChange {
            key: Some(WindowKey { pid: 2, id: 2 }),
            focus: Some(FocusSnapshot {
                id: 2,
                app: "New".into(),
                title: "Second".into(),
                pid: 2,
                display_id: None,
            }),
        }));

        tokio::time::timeout(Duration::from_millis(200), async {
            loop {
                if engine
                    .current_focus_snapshot()
                    .is_some_and(|focus| focus.pid == 2)
                {
                    return;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("engine did not observe final focus event");

        let focus = engine
            .current_focus_snapshot()
            .expect("final focus snapshot");
        assert_eq!(
            focus,
            FocusSnapshot {
                id: 2,
                app: "New".into(),
                title: "Second".into(),
                pid: 2,
                display_id: None,
            }
        );
    }

    #[tokio::test]
    async fn focus_cache_preserves_complete_snapshots_and_none() {
        let world = Arc::new(TestWorld::new());
        let (tx, _rx) = mpsc::channel(128);
        let engine = Engine::build(Arc::new(MockHotkeyApi::new()), tx, false, false, world);
        assert_eq!(engine.current_focus_snapshot(), None);

        let focus = FocusSnapshot {
            id: 77,
            app: "Editor".into(),
            title: "Notes".into(),
            pid: 9,
            display_id: Some(4),
        };
        engine
            .apply_world_focus_snapshot(Some(focus.clone()))
            .await
            .expect("install focus snapshot");
        assert_eq!(engine.current_focus_snapshot(), Some(focus));

        engine
            .apply_world_focus_snapshot(None)
            .await
            .expect("clear focus snapshot");
        assert_eq!(engine.current_focus_snapshot(), None);
    }

    #[tokio::test]
    async fn background_tasks_release_with_last_engine_owner() {
        let world = Arc::new(TestWorld::new());
        let (tx, _rx) = mpsc::channel(16);
        let engine =
            Engine::new_with_api_and_world(Arc::new(MockHotkeyApi::new()), tx, false, world);
        let lifecycle = engine.lifecycle.weak();
        let cancellation = engine.background_cancellation_token();
        let remaining = engine.clone();

        assert_eq!(engine.lifecycle.task_count(), 2);
        drop(engine);
        assert!(lifecycle.upgrade().is_some());
        assert!(!cancellation.is_cancelled());

        drop(remaining);

        assert!(lifecycle.upgrade().is_none());
        assert!(cancellation.is_cancelled());
    }
}
