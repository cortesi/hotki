use std::time::{Duration, Instant};

use config::runtime as dyn_engine;
use mac_keycode::Chord;
use tracing::{trace, warn};

use crate::{DispatchResult, Engine, HeldBinding, Result, selector_controller::SelectorController};

impl Engine {
    async fn handle_key_event(
        &self,
        chord: &mac_keycode::Chord,
        identifier: &str,
        refresh_world: bool,
    ) -> Result<()> {
        let start = Instant::now();
        if refresh_world && self.sync_on_dispatch {
            self.refresh_world_focus().await?;
        }
        let focus = self.current_focus_snapshot();

        trace!(identifier, focus = ?focus, "Key event received");

        if self.config.lock().await.is_none() {
            trace!("No dynamic config loaded; ignoring key");
            return Ok(());
        }

        if SelectorController::new(self)
            .handle_input(chord, identifier, &focus)
            .await?
        {
            trace!(
                "Selector key event completed in {:?}: {}",
                start.elapsed(),
                identifier
            );
            return Ok(());
        }

        let (binding, ctx) = {
            let rt = self.runtime.lock().await;
            let Some(binding) =
                dyn_engine::ConfigRuntime::resolve_binding(&rt.rendered, chord).cloned()
            else {
                trace!("No binding for chord {}", chord);
                return Ok(());
            };
            let ctx = rt.mode_ctx(&focus);
            (binding, ctx)
        };

        let Some(result) = self.execute_binding(identifier, binding, ctx).await? else {
            return Ok(());
        };

        if result.should_auto_exit() {
            self.auto_exit().await;
        }

        self.rebind_and_refresh(&focus).await?;
        trace!(
            "Key event completed in {:?}: {}",
            start.elapsed(),
            identifier
        );
        Ok(())
    }

    async fn execute_binding(
        &self,
        identifier: &str,
        binding: dyn_engine::Binding,
        ctx: dyn_engine::ModeCtx,
    ) -> Result<Option<DispatchResult>> {
        let stays_in_mode = binding.stays_in_mode();
        let result = match binding.kind {
            dyn_engine::BindingKind::Mode(mode) => {
                let mut rt = self.runtime.lock().await;
                rt.push_mode(
                    binding.desc,
                    mode,
                    binding.mode_id.map(|id| (binding.chord, id)),
                    binding.mode_capture,
                    ctx.window,
                );
                DispatchResult::EnteredMode
            }
            dyn_engine::BindingKind::Handler(handler) => {
                let result = {
                    let mut cfg_guard = self.config.lock().await;
                    let Some(cfg) = cfg_guard.as_mut() else {
                        trace!("No dynamic config loaded; ignoring handler");
                        return Ok(None);
                    };
                    match cfg.execute_handler(&handler, &ctx) {
                        Ok(result) => result,
                        Err(err) => {
                            self.notifier.send_error("Handler", err.pretty())?;
                            return Ok(None);
                        }
                    }
                };

                self.apply_effects(identifier, result.effects, ctx)
                    .await?
                    .with_stay(result.stay)
            }
        };

        Ok(Some(result.with_stay(stays_in_mode)))
    }

    async fn handle_key_up(&self, identifier: &str) {
        self.action_repeater.stop(identifier).await;
        self.repeater.stop(identifier).await;
        if self.relay.stop_relay(identifier) {
            tracing::debug!("Stopped relay for {}", identifier);
        }
    }

    async fn handle_repeat(&self, identifier: &str) {
        if self.relay.repeat_relay(identifier) {
            if self.repeater.is_ticking(identifier) {
                self.repeater.note_os_repeat(identifier).await;
            }
            tracing::debug!("Repeated relay for {}", identifier);
        }
    }

    /// Dispatch a hotkey event by id, handling all lookups and callback execution internally.
    /// This reduces the server's knowledge about engine internals and avoids repeated async locking.
    pub async fn dispatch(&self, id: u32, kind: mac_hotkey::EventKind, repeat: bool) -> Result<()> {
        let start = Instant::now();
        let binding = match self.resolve_dispatch_binding(id, kind, repeat).await {
            Some(binding) => binding,
            None => {
                trace!("Dispatch called with unregistered id: {}", id);
                return Ok(());
            }
        };

        let result = self
            .dispatch_resolved(&binding.identifier, &binding.chord, kind, repeat, true)
            .await;
        let elapsed = start.elapsed();
        if elapsed > Duration::from_millis(crate::KEY_DISPATCH_WARN_MS) {
            warn!(
                "Key dispatch took {:?} for {} {:?} (repeat: {})",
                elapsed, binding.identifier, kind, repeat
            );
        }
        result
    }

    async fn resolve_dispatch_binding(
        &self,
        id: u32,
        kind: mac_hotkey::EventKind,
        repeat: bool,
    ) -> Option<HeldBinding> {
        match kind {
            mac_hotkey::EventKind::KeyUp => {
                if let Some(binding) = self.held_bindings.lock().remove(&id) {
                    return Some(binding);
                }
            }
            mac_hotkey::EventKind::KeyDown if repeat => {
                if let Some(binding) = self.held_bindings.lock().get(&id).cloned() {
                    return Some(binding);
                }
            }
            mac_hotkey::EventKind::KeyDown => {}
        }

        let (identifier, chord) = self.binding_manager.lock().await.resolve(id)?;
        let binding = HeldBinding { identifier, chord };
        if matches!(kind, mac_hotkey::EventKind::KeyDown) {
            self.held_bindings.lock().insert(id, binding.clone());
        }
        Some(binding)
    }

    /// Inject a hotkey event by identifier using the event-maintained focus cache.
    ///
    /// Synthetic callers do not wait for a platform focus capture. Physical input
    /// should use [`Self::dispatch`], which establishes a fresh focus generation.
    pub async fn dispatch_injected(
        &self,
        ident: &str,
        kind: mac_hotkey::EventKind,
        repeat: bool,
    ) -> Result<bool> {
        let Some(chord) = self.binding_manager.lock().await.chord_for_ident(ident) else {
            return Ok(false);
        };
        self.dispatch_resolved(ident, &chord, kind, repeat, false)
            .await?;
        Ok(true)
    }

    async fn dispatch_resolved(
        &self,
        ident: &str,
        chord: &Chord,
        kind: mac_hotkey::EventKind,
        repeat: bool,
        refresh_world: bool,
    ) -> Result<()> {
        trace!("Key event: {} {:?} (repeat: {})", ident, kind, repeat);

        match kind {
            mac_hotkey::EventKind::KeyDown => {
                if repeat {
                    if self.runtime.lock().await.selector.is_some() {
                        if let Err(error) = self.handle_key_event(chord, ident, refresh_world).await
                        {
                            warn!("Key handler failed: {}", error);
                            return Err(error);
                        }
                        return Ok(());
                    }
                    if self.key_tracker.is_down(ident) && self.key_tracker.is_repeat_allowed(ident)
                    {
                        self.handle_repeat(ident).await;
                    }
                    return Ok(());
                }

                self.key_tracker.on_key_down(ident);

                if let Err(error) = self.handle_key_event(chord, ident, refresh_world).await {
                    warn!("Key handler failed: {}", error);
                    return Err(error);
                }
            }
            mac_hotkey::EventKind::KeyUp => {
                self.key_tracker.on_key_up(ident);
                self.handle_key_up(ident).await;
            }
        }
        Ok(())
    }

    /// Resolve a registration id for an identifier (e.g., "cmd+k"). Intended for diagnostics/tests.
    pub async fn resolve_id_for_ident(&self, ident: &str) -> Option<u32> {
        self.binding_manager.lock().await.id_for_ident(ident)
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, sync::Arc};

    use hotki_protocol::MsgToUI;
    use hotki_world::{TestWorld, WindowKey, WorldWindow};
    use tokio::sync::mpsc;

    use super::*;
    use crate::deps::MockHotkeyApi;

    #[tokio::test]
    async fn dispatch_refreshes_focus_before_resolving_contextual_binding() {
        let world = Arc::new(TestWorld::new());
        world.set_snapshot(
            vec![WorldWindow {
                app: "Old".into(),
                title: "First".into(),
                pid: 1,
                id: 1,
                display_id: None,
                focused: true,
            }],
            Some(WindowKey { pid: 1, id: 1 }),
        );
        let (tx, mut rx) = mpsc::channel(128);
        let engine = Engine::build(
            Arc::new(MockHotkeyApi::new()),
            tx,
            false,
            true,
            world.clone(),
        );
        engine
            .refresh_world_focus()
            .await
            .expect("seed focus context");

        let path = crate::test_support::write_test_config(
            r#"
            local a = hotki.actions
            return function(menu, ctx)
                local window = ctx.window
                if window ~= nil and window:app_matches("New") then
                    menu:bind("a", "new", function(action_ctx)
                        local captured = action_ctx.window
                        if captured == nil then error("dispatch lost captured window") end
                        action_ctx:notify("info", "Dispatch", tostring(captured.id))
                    end)
                else
                    menu:bind("a", "old", a.notify("info", "Dispatch", "old"))
                end
            end
            "#,
        );
        engine
            .set_config_path(path.clone())
            .await
            .expect("set config");
        let id = engine
            .resolve_id_for_ident("a")
            .await
            .expect("registered binding");
        while rx.try_recv().is_ok() {}

        world.set_snapshot(
            vec![WorldWindow {
                app: "New".into(),
                title: "Second".into(),
                pid: 2,
                id: 2,
                display_id: None,
                focused: true,
            }],
            Some(WindowKey { pid: 2, id: 2 }),
        );
        engine
            .dispatch(id, mac_hotkey::EventKind::KeyDown, false)
            .await
            .expect("dispatch key down");

        assert!(
            crate::test_support::recv_until(&mut rx, 200, |message| matches!(
                message,
                MsgToUI::Notify { title, text, .. }
                    if title == "Dispatch" && text == "2"
            ))
            .await,
            "dispatch should use the focus state refreshed in the same call"
        );

        let _ignored = fs::remove_file(path);
    }
}
