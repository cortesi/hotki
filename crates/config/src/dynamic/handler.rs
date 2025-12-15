use rhai::Dynamic;

use super::{ActionCtx, DynamicConfig, HandlerRef, ModeCtx, NavRequest};
use crate::Error;

/// Result of executing a handler closure.
#[derive(Debug)]
pub struct HandlerResult {
    pub effects: Vec<super::Effect>,
    pub nav: Option<NavRequest>,
    pub stay: bool,
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
        .call::<Dynamic>(&cfg.engine, &cfg.ast, (action_ctx.clone(),))
        .map(|_| ())
        .map_err(|err| super::render::rhai_error_to_config(cfg, &err))?;

    Ok(HandlerResult {
        effects: action_ctx.take_effects(),
        nav: action_ctx.take_nav(),
        stay: action_ctx.stay(),
    })
}
