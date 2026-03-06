use config::dynamic::engine as dyn_engine;
use tracing::{trace, warn};

use crate::{DispatchOutcome, Engine, Result, selector_controller::SelectorController};

struct BindingExecution {
    stay: bool,
    outcome: DispatchOutcome,
}

impl Engine {
    async fn handle_key_event(&self, chord: &mac_keycode::Chord, identifier: String) -> Result<()> {
        let start = std::time::Instant::now();
        if self.sync_on_dispatch {
            self.world.hint_refresh();
        }
        let dispatch_ctx = self.current_dispatch_context();

        trace!(
            "Key event received: {} (app: {}, title: {})",
            identifier, dispatch_ctx.app, dispatch_ctx.title
        );

        if self.config.read().await.is_none() {
            trace!("No dynamic config loaded; ignoring key");
            return Ok(());
        }

        if SelectorController::new(self)
            .handle_input(chord, identifier.as_str(), &dispatch_ctx)
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
            let Some(binding) = dyn_engine::resolve_binding(&rt.rendered, chord).cloned() else {
                trace!("No binding for chord {}", chord);
                return Ok(());
            };
            let ctx = dispatch_ctx.mode_ctx(&rt);
            (binding, ctx)
        };

        let Some(execution) = self
            .execute_binding(identifier.as_str(), binding, ctx)
            .await?
        else {
            return Ok(());
        };

        if !execution.stay && !execution.outcome.is_nav && !execution.outcome.entered_mode {
            self.auto_exit().await;
        }

        let processing_time = start.elapsed();
        if processing_time > std::time::Duration::from_millis(crate::KEY_PROC_WARN_MS) {
            warn!(
                "Key processing took {:?} for {}",
                processing_time, identifier
            );
        }

        self.rebind_and_refresh(dispatch_ctx).await?;
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
    ) -> Result<Option<BindingExecution>> {
        let mut stay = binding.flags.stay;
        let mut outcome = DispatchOutcome::default();

        match binding.kind.clone() {
            dyn_engine::BindingKind::Mode(mode) => {
                outcome.entered_mode = true;
                let mut rt = self.runtime.lock().await;
                rt.hud_visible = true;
                rt.stack.push(dyn_engine::ModeFrame {
                    title: binding.desc,
                    closure: mode,
                    entered_via: binding.mode_id.map(|id| (binding.chord, id)),
                    rendered: Vec::new(),
                    style: None,
                    capture: binding.mode_capture,
                });
            }
            dyn_engine::BindingKind::Action(action) => {
                outcome = self
                    .apply_action(identifier, &action, binding.flags.repeat)
                    .await?;
            }
            dyn_engine::BindingKind::Handler(handler) => {
                let result = {
                    let cfg_guard = self.config.read().await;
                    let Some(cfg) = cfg_guard.as_ref() else {
                        trace!("No dynamic config loaded; ignoring handler");
                        return Ok(None);
                    };
                    match dyn_engine::execute_handler(cfg, &handler, &ctx) {
                        Ok(result) => result,
                        Err(err) => {
                            self.notifier.send_error("Handler", err.pretty())?;
                            return Ok(None);
                        }
                    }
                };

                stay |= result.stay;
                outcome = self
                    .apply_effects_and_nav(identifier, result.effects, result.nav)
                    .await?;
            }
            dyn_engine::BindingKind::Selector(sel_cfg) => {
                stay = true;
                let items = {
                    let cfg_guard = self.config.read().await;
                    let Some(cfg) = cfg_guard.as_ref() else {
                        trace!("No dynamic config loaded; ignoring selector");
                        return Ok(None);
                    };
                    match sel_cfg.resolve_items(cfg, &ctx) {
                        Ok(items) => items,
                        Err(err) => {
                            self.notifier.send_error("Selector", err.pretty())?;
                            Vec::new()
                        }
                    }
                };

                let snapshot = {
                    let notify = self.selector_notify.clone();
                    let notify_cb: std::sync::Arc<dyn Fn() + Send + Sync> =
                        std::sync::Arc::new(move || notify.notify_one());
                    let mut rt = self.runtime.lock().await;
                    let prev_hud_visible = rt.hud_visible;
                    rt.hud_visible = false;
                    rt.selector = Some(crate::selector::SelectorState::new(
                        sel_cfg,
                        items,
                        notify_cb,
                        prev_hud_visible,
                    ));
                    let selector = rt.selector.as_mut().expect("selector just set");
                    let _changed_ignored = selector.tick();
                    crate::selector_controller::selector_snapshot_for_ui(selector)
                };
                self.notifier
                    .send_ui(hotki_protocol::MsgToUI::SelectorUpdate(snapshot))?;
            }
        }

        Ok(Some(BindingExecution { stay, outcome }))
    }

    fn handle_key_up(&self, identifier: &str) {
        let pid = self.current_dispatch_context().pid;
        self.repeater.stop_sync(identifier);
        if self.relay.stop_relay(identifier, pid) {
            tracing::debug!("Stopped relay for {}", identifier);
        }
    }

    fn handle_repeat(&self, identifier: &str) {
        let pid = self.current_dispatch_context().pid;
        if self.relay.repeat_relay(identifier, pid) {
            if self.repeater.is_ticking(identifier) {
                self.repeater.note_os_repeat(identifier);
            }
            tracing::debug!("Repeated relay for {}", identifier);
        }
    }

    /// Dispatch a hotkey event by id, handling all lookups and callback execution internally.
    /// This reduces the server's knowledge about engine internals and avoids repeated async locking.
    pub async fn dispatch(&self, id: u32, kind: mac_hotkey::EventKind, repeat: bool) -> Result<()> {
        let (ident, chord) = match self.binding_manager.lock().await.resolve(id) {
            Some((i, c)) => (i, c),
            None => {
                trace!("Dispatch called with unregistered id: {}", id);
                return Ok(());
            }
        };

        trace!("Key event: {} {:?} (repeat: {})", ident, kind, repeat);

        match kind {
            mac_hotkey::EventKind::KeyDown => {
                if repeat {
                    if self.runtime.lock().await.selector.is_some() {
                        if let Err(error) = self.handle_key_event(&chord, ident.clone()).await {
                            warn!("Key handler failed: {}", error);
                            return Err(error);
                        }
                        return Ok(());
                    }
                    if self.key_tracker.is_down(&ident)
                        && self.key_tracker.is_repeat_allowed(&ident)
                    {
                        self.handle_repeat(&ident);
                    }
                    return Ok(());
                }

                self.key_tracker.on_key_down(&ident);

                if let Err(error) = self.handle_key_event(&chord, ident.clone()).await {
                    warn!("Key handler failed: {}", error);
                    return Err(error);
                }
            }
            mac_hotkey::EventKind::KeyUp => {
                self.key_tracker.on_key_up(&ident);
                self.handle_key_up(&ident);
            }
        }
        Ok(())
    }

    /// Resolve a registration id for an identifier (e.g., "cmd+k"). Intended for diagnostics/tests.
    pub async fn resolve_id_for_ident(&self, ident: &str) -> Option<u32> {
        self.binding_manager.lock().await.id_for_ident(ident)
    }
}
