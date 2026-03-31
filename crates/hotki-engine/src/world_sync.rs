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
        let engine = self.clone();
        tokio::spawn(async move {
            loop {
                let (mut cursor, seed) = hotki_world::subscribe_with_snapshot(world.as_ref()).await;
                if let Err(err) = engine.apply_world_focus_snapshot(seed).await {
                    warn!("World focus seed apply failed: {}", err);
                }

                let mut last_lost = cursor.lost_count;
                loop {
                    let deadline =
                        tokio::time::Instant::now() + tokio::time::Duration::from_secs(30);
                    match world.next_event_until(&mut cursor, deadline).await {
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
                                let world_clone = world.clone();
                                let engine_clone = engine.clone();
                                tokio::spawn(async move {
                                    Engine::handle_focus_change_event(
                                        engine_clone,
                                        world_clone,
                                        change,
                                    )
                                    .await;
                                });
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
    }

    pub(crate) fn spawn_selector_notify_task(&self) {
        let engine = self.clone();
        let notify = self.selector_notify.clone();
        tokio::spawn(async move {
            loop {
                notify.notified().await;
                if let Err(err) = engine.on_selector_notify().await {
                    warn!("Selector notify tick failed: {}", err);
                }
            }
        });
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

    async fn handle_focus_change_event(
        engine: Engine,
        world: Arc<dyn WorldView>,
        change: FocusChange,
    ) {
        let focus = hotki_world::focus_snapshot_for_change(world.as_ref(), &change).await;

        if let Some(focus) = focus {
            if let Err(err) = engine.apply_world_focus_snapshot(Some(focus)).await {
                warn!("World focus update failed: {}", err);
            }
        } else if change.key.is_none() {
            if let Err(err) = engine.apply_world_focus_snapshot(None).await {
                warn!("World focus clear failed: {}", err);
            }
        } else {
            warn!(key = ?change.key, "World focus context unavailable after focus change");
        }
    }

    pub(crate) async fn rebind_current_context(&self) -> Result<()> {
        let focus = self.current_focus_info();
        debug!(
            "Rebinding with context: app={}, title={}",
            focus.app, focus.title
        );
        self.rebind_and_refresh(focus).await
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

    pub(crate) fn current_focus_info(&self) -> FocusInfo {
        if let Some(focus) = &*self.focus_ctx.lock() {
            return FocusInfo {
                app: focus.app.clone(),
                title: focus.title.clone(),
                pid: focus.pid,
            };
        }
        FocusInfo {
            pid: -1,
            ..FocusInfo::default()
        }
    }
}
