use std::time::{Duration, Instant};

use config::script::engine as dyn_engine;
use mac_keycode::Chord;
use tracing::{trace, warn};

use crate::{DispatchResult, Engine, Result, selector_controller::SelectorController};

impl Engine {
    async fn handle_key_event(&self, chord: &mac_keycode::Chord, identifier: &str) -> Result<()> {
        let start = Instant::now();
        if self.sync_on_dispatch {
            self.world.hint_refresh();
        }
        let focus = self.current_focus_info();

        trace!(
            "Key event received: {} (app: {}, title: {})",
            identifier, focus.app, focus.title
        );

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
            let Some(binding) = dyn_engine::resolve_binding(&rt.rendered, chord).cloned() else {
                trace!("No binding for chord {}", chord);
                return Ok(());
            };
            let ctx = rt.focus.mode_ctx(rt.hud_visible, rt.depth());
            (binding, ctx)
        };

        let Some(result) = self.execute_binding(identifier, binding, ctx).await? else {
            return Ok(());
        };

        if result.should_auto_exit() {
            self.auto_exit().await;
        }

        let processing_time = start.elapsed();
        if processing_time > Duration::from_millis(crate::KEY_PROC_WARN_MS) {
            warn!(
                "Key processing took {:?} for {}",
                processing_time, identifier
            );
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
        let result = match binding.kind {
            dyn_engine::BindingKind::Mode(mode) => {
                let mut rt = self.runtime.lock().await;
                rt.hud_visible = true;
                rt.stack.push(dyn_engine::ModeFrame {
                    title: binding.desc,
                    closure: mode,
                    entered_via: binding.mode_id.map(|id| (binding.chord, id)),
                    rendered: Vec::new(),
                    capture: binding.mode_capture,
                });
                DispatchResult::EnteredMode
            }
            dyn_engine::BindingKind::Action(action) => {
                self.apply_action(identifier, &action, binding.flags.repeat)
                    .await?
            }
            dyn_engine::BindingKind::Handler(handler) => {
                let result = {
                    let mut cfg_guard = self.config.lock().await;
                    let Some(cfg) = cfg_guard.as_mut() else {
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

                self.apply_effects_and_nav(identifier, result.effects, result.nav)
                    .await?
                    .with_stay(result.stay)
            }
            dyn_engine::BindingKind::Selector(sel_cfg) => {
                if !SelectorController::new(self).open(sel_cfg, ctx).await? {
                    return Ok(None);
                }
                DispatchResult::SelectorOpened
            }
        };

        Ok(Some(result.with_stay(binding.flags.stay)))
    }

    fn handle_key_up(&self, identifier: &str) {
        let pid = self.current_focus_info().pid;
        self.repeater.stop_sync(identifier);
        if self.relay.stop_relay(identifier, pid) {
            tracing::debug!("Stopped relay for {}", identifier);
        }
    }

    fn handle_repeat(&self, identifier: &str) {
        if self.relay.repeat_relay(identifier) {
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

        self.dispatch_resolved(ident, chord, kind, repeat).await
    }

    /// Dispatch a hotkey event by identifier, returning false when the binding is absent.
    pub async fn dispatch_ident(
        &self,
        ident: &str,
        kind: mac_hotkey::EventKind,
        repeat: bool,
    ) -> Result<bool> {
        let Some(chord) = self.binding_manager.lock().await.chord_for_ident(ident) else {
            return Ok(false);
        };
        self.dispatch_resolved(ident.to_string(), chord, kind, repeat)
            .await?;
        Ok(true)
    }

    async fn dispatch_resolved(
        &self,
        ident: String,
        chord: Chord,
        kind: mac_hotkey::EventKind,
        repeat: bool,
    ) -> Result<()> {
        trace!("Key event: {} {:?} (repeat: {})", ident, kind, repeat);

        match kind {
            mac_hotkey::EventKind::KeyDown => {
                if repeat {
                    if self.runtime.lock().await.selector.is_some() {
                        if let Err(error) = self.handle_key_event(&chord, &ident).await {
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

                if let Err(error) = self.handle_key_event(&chord, &ident).await {
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
