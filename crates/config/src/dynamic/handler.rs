use rhai::EvalAltResult;

use crate::Error;

use super::{ActionCtx, HandlerRef, ModeCtx, NavRequest};

use super::DynamicConfig;

/// Result of executing a handler closure.
#[derive(Debug)]
pub struct HandlerResult {
    pub(crate) effects: Vec<super::Effect>,
    pub(crate) nav: Option<NavRequest>,
    pub(crate) stay: bool,
}

pub fn execute_handler(
    cfg: &DynamicConfig,
    handler: &HandlerRef,
    ctx: &ModeCtx,
) -> Result<HandlerResult, Error> {
    let action_ctx = ActionCtx::new(
        ctx.app.clone(),
        ctx.title.clone(),
        ctx.pid,
        ctx.hud,
        ctx.depth,
    );

    handler
        .func
        .call::<()>(&cfg.engine, &cfg.ast, (action_ctx.clone(),))
        .map_err(|err| rhai_error_to_config(cfg, &err))?;

    Ok(HandlerResult {
        effects: action_ctx.take_effects(),
        nav: action_ctx.take_nav(),
        stay: action_ctx.stay(),
    })
}

fn rhai_error_to_config(cfg: &DynamicConfig, err: &EvalAltResult) -> Error {
    super::render::rhai_error_to_config(cfg, err)
}

