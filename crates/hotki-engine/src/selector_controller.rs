use std::{result, sync::Arc};

use config::script::engine as dyn_engine;

use crate::{
    Engine, Result,
    runtime::FocusInfo,
    selector::{SelectorEvent, SelectorSelection, SelectorState},
};

/// Selector close request carrying only the data needed to run terminal handlers.
#[derive(Debug)]
struct SelectorClose {
    /// Terminal selector action.
    terminal: SelectorTerminal,
    /// Selector configuration owning terminal handlers.
    config: dyn_engine::SelectorConfig,
    /// Mode context captured when the selector closed.
    ctx: dyn_engine::ModeCtx,
}

/// Terminal selector action.
#[derive(Debug)]
enum SelectorTerminal {
    /// User selected an item.
    Select(SelectorSelection),
    /// User canceled the selector.
    Cancel,
}

/// Input handling result for one selector key event.
enum SelectorInput {
    /// No selector is active.
    Inactive,
    /// Input was handled without a UI update.
    Consumed,
    /// Input changed selector state and produced a new snapshot.
    Update(hotki_protocol::SelectorSnapshot),
    /// Input closed the selector.
    Close(Box<SelectorClose>),
}

/// Controller for opening, updating, and closing selector UI state.
pub(crate) struct SelectorController<'a> {
    /// Engine that owns selector runtime state and notification channels.
    engine: &'a Engine,
}

impl<'a> SelectorController<'a> {
    /// Construct a controller for an engine.
    pub(crate) fn new(engine: &'a Engine) -> Self {
        Self { engine }
    }

    /// Resolve selector items, install selector state, and publish the initial snapshot.
    pub(crate) async fn open(
        &self,
        config: dyn_engine::SelectorConfig,
        ctx: dyn_engine::ModeCtx,
    ) -> Result<bool> {
        let items = {
            let mut cfg_guard = self.engine.config.lock().await;
            let Some(cfg) = cfg_guard.as_mut() else {
                tracing::trace!("No dynamic config loaded; ignoring selector");
                return Ok(false);
            };
            match config.resolve_items(cfg, &ctx) {
                Ok(items) => items,
                Err(err) => {
                    self.engine.notifier.send_error("Selector", err.pretty())?;
                    Vec::new()
                }
            }
        };

        let snapshot = {
            let notify = self.engine.selector_notify.clone();
            let notify_cb: Arc<dyn Fn() + Send + Sync> = Arc::new(move || notify.notify_one());
            let mut rt = self.engine.runtime.lock().await;
            let prev_hud_visible = rt.hud_visible;
            rt.hud_visible = false;
            let mut selector = SelectorState::new(config, items, notify_cb, prev_hud_visible);
            let _changed_ignored = selector.tick();
            let snapshot = selector.snapshot();
            rt.selector = Some(selector);
            snapshot
        };

        self.engine
            .notifier
            .send_ui(hotki_protocol::MsgToUI::SelectorUpdate(snapshot))?;
        Ok(true)
    }

    /// Route one key event to an active selector, returning true if consumed.
    pub(crate) async fn handle_input(
        &self,
        chord: &mac_keycode::Chord,
        identifier: &str,
        focus: &FocusInfo,
    ) -> Result<bool> {
        match self.selector_input(chord).await {
            SelectorInput::Inactive => Ok(false),
            SelectorInput::Consumed => Ok(true),
            SelectorInput::Update(snapshot) => {
                self.engine
                    .notifier
                    .send_ui(hotki_protocol::MsgToUI::SelectorUpdate(snapshot))?;
                Ok(true)
            }
            SelectorInput::Close(close) => {
                self.complete_close(identifier, focus, *close).await?;
                Ok(true)
            }
        }
    }

    /// Apply one key event to the active selector state.
    async fn selector_input(&self, chord: &mac_keycode::Chord) -> SelectorInput {
        let mut rt = self.engine.runtime.lock().await;
        let Some(mut selector) = rt.selector.take() else {
            return SelectorInput::Inactive;
        };

        let event = selector.handle_key_down(chord);
        match event {
            SelectorEvent::Update => {
                let _changed_ignored = selector.tick();
                let snapshot = selector.snapshot();
                rt.selector = Some(selector);
                SelectorInput::Update(snapshot)
            }
            SelectorEvent::Select(selection) => {
                rt.hud_visible = selector.prev_hud_visible;
                SelectorInput::Close(Box::new(SelectorClose {
                    terminal: SelectorTerminal::Select(selection),
                    ctx: rt.focus.mode_ctx(rt.hud_visible, rt.depth()),
                    config: selector.config,
                }))
            }
            SelectorEvent::Cancel => {
                rt.hud_visible = selector.prev_hud_visible;
                SelectorInput::Close(Box::new(SelectorClose {
                    terminal: SelectorTerminal::Cancel,
                    ctx: rt.focus.mode_ctx(rt.hud_visible, rt.depth()),
                    config: selector.config,
                }))
            }
            SelectorEvent::None => {
                rt.selector = Some(selector);
                SelectorInput::Consumed
            }
        }
    }

    /// Publish close UI and execute the configured terminal handler.
    async fn complete_close(
        &self,
        identifier: &str,
        focus: &FocusInfo,
        close: SelectorClose,
    ) -> Result<()> {
        self.engine
            .notifier
            .send_ui(hotki_protocol::MsgToUI::SelectorHide)?;

        let result = {
            let mut cfg_guard = self.engine.config.lock().await;
            let Some(cfg) = cfg_guard.as_mut() else {
                tracing::trace!("No dynamic config loaded; ignoring selector close");
                self.engine.rebind_and_refresh(focus.clone()).await?;
                return Ok(());
            };
            execute_selector_close(cfg, &close)
        };

        let result = match result {
            Ok(result) => result,
            Err(err) => {
                self.engine.notifier.send_error("Selector", err.pretty())?;
                self.engine.rebind_and_refresh(focus.clone()).await?;
                return Ok(());
            }
        };

        let _outcome_ignored = self
            .engine
            .apply_effects_and_nav(identifier, result.effects, result.nav)
            .await?;
        self.engine.rebind_and_refresh(focus.clone()).await
    }
}

/// Execute the close handler described by a terminal selector event.
fn execute_selector_close(
    cfg: &mut dyn_engine::DynamicConfig,
    close: &SelectorClose,
) -> result::Result<dyn_engine::HandlerResult, config::Error> {
    match &close.terminal {
        SelectorTerminal::Select(selection) => dyn_engine::execute_selector_handler(
            cfg,
            &close.config.on_select,
            &close.ctx,
            &selection.item,
            &selection.query,
        ),
        SelectorTerminal::Cancel => match close.config.on_cancel.as_ref() {
            Some(handler) => dyn_engine::execute_handler(cfg, handler, &close.ctx),
            None => Ok(selector_noop_result()),
        },
    }
}

/// Empty handler result used when a selector terminal action has no handler work.
fn selector_noop_result() -> dyn_engine::HandlerResult {
    dyn_engine::HandlerResult {
        effects: Vec::new(),
        nav: None,
        stay: true,
    }
}
