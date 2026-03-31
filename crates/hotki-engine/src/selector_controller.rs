use config::script::engine as dyn_engine;

use crate::{
    DispatchContext, Engine, Result,
    selector::{SelectorEvent, SelectorState},
};

#[derive(Debug)]
struct SelectorClose {
    event: SelectorEvent,
    selector: SelectorState,
    ctx: dyn_engine::ModeCtx,
}

enum SelectorInput {
    Inactive,
    Consumed,
    Update(hotki_protocol::SelectorSnapshot),
    Close(Box<SelectorClose>),
}

pub(crate) struct SelectorController<'a> {
    engine: &'a Engine,
}

impl<'a> SelectorController<'a> {
    pub(crate) fn new(engine: &'a Engine) -> Self {
        Self { engine }
    }

    pub(crate) async fn handle_input(
        &self,
        chord: &mac_keycode::Chord,
        identifier: &str,
        dispatch_ctx: &DispatchContext,
    ) -> Result<bool> {
        match self.selector_input(chord, dispatch_ctx).await {
            SelectorInput::Inactive => Ok(false),
            SelectorInput::Consumed => Ok(true),
            SelectorInput::Update(snapshot) => {
                self.engine
                    .notifier
                    .send_ui(hotki_protocol::MsgToUI::SelectorUpdate(snapshot))?;
                Ok(true)
            }
            SelectorInput::Close(close) => {
                self.complete_close(identifier, dispatch_ctx, *close)
                    .await?;
                Ok(true)
            }
        }
    }

    async fn selector_input(
        &self,
        chord: &mac_keycode::Chord,
        dispatch_ctx: &DispatchContext,
    ) -> SelectorInput {
        let mut rt = self.engine.runtime.lock().await;
        let Some(selector) = rt.selector.as_mut() else {
            return SelectorInput::Inactive;
        };

        let event = selector.handle_key_down(chord);
        match event {
            SelectorEvent::Update => {
                let _changed_ignored = selector.tick();
                SelectorInput::Update(selector.snapshot())
            }
            SelectorEvent::Select | SelectorEvent::Cancel => {
                let selector = rt.selector.take().expect("selector must exist for close");
                rt.hud_visible = selector.prev_hud_visible;
                SelectorInput::Close(Box::new(SelectorClose {
                    event,
                    ctx: dispatch_ctx.mode_ctx(&rt),
                    selector,
                }))
            }
            SelectorEvent::None => SelectorInput::Consumed,
        }
    }

    async fn complete_close(
        &self,
        identifier: &str,
        dispatch_ctx: &DispatchContext,
        mut close: SelectorClose,
    ) -> Result<()> {
        self.engine
            .notifier
            .send_ui(hotki_protocol::MsgToUI::SelectorHide)?;

        let result = {
            let cfg_guard = self.engine.config.read().await;
            let Some(cfg) = cfg_guard.as_ref() else {
                tracing::trace!("No dynamic config loaded; ignoring selector close");
                self.engine.rebind_and_refresh(dispatch_ctx.clone()).await?;
                return Ok(());
            };
            execute_selector_close(cfg, &mut close)
        };

        let result = match result {
            Ok(result) => result,
            Err(err) => {
                self.engine.notifier.send_error("Selector", err.pretty())?;
                self.engine.rebind_and_refresh(dispatch_ctx.clone()).await?;
                return Ok(());
            }
        };

        let _outcome_ignored = self
            .engine
            .apply_effects_and_nav(identifier, result.effects, result.nav)
            .await?;
        self.engine.rebind_and_refresh(dispatch_ctx.clone()).await
    }
}

fn execute_selector_close(
    cfg: &dyn_engine::DynamicConfig,
    close: &mut SelectorClose,
) -> std::result::Result<dyn_engine::HandlerResult, config::Error> {
    match close.event {
        SelectorEvent::Select => {
            let _changed_ignored = close.selector.tick();
            let Some(item) = close.selector.selected_item() else {
                return Ok(selector_noop_result());
            };
            dyn_engine::execute_selector_handler(
                cfg,
                &close.selector.config.on_select,
                &close.ctx,
                &item,
                close.selector.query(),
            )
        }
        SelectorEvent::Cancel => match close.selector.config.on_cancel.as_ref() {
            Some(handler) => dyn_engine::execute_handler(cfg, handler, &close.ctx),
            None => Ok(selector_noop_result()),
        },
        SelectorEvent::Update | SelectorEvent::None => unreachable!("selector close is terminal"),
    }
}

fn selector_noop_result() -> dyn_engine::HandlerResult {
    dyn_engine::HandlerResult {
        effects: Vec::new(),
        nav: None,
        stay: true,
    }
}
