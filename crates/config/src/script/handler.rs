use mlua::{AnyUserData, Value};

use super::{ActionCtx, DynamicConfig, HandlerRef, ModeCtx, NavRequest, SelectorItem};
use crate::Error;

/// Result of executing a handler closure.
#[derive(Debug)]
pub struct HandlerResult {
    /// Side effects queued by the handler.
    pub effects: Vec<super::Effect>,
    /// Optional navigation request emitted by the handler.
    pub nav: Option<NavRequest>,
    /// True when the handler requested to suppress auto-exit behavior.
    pub stay: bool,
}

/// Reset the budget, build an `ActionCtx`, and create its Lua userdata value.
fn prepare_handler_call(
    cfg: &DynamicConfig,
    ctx: &ModeCtx,
) -> Result<(ActionCtx, AnyUserData), Error> {
    cfg.reset_execution_budget();
    let action_ctx = ActionCtx::new(ctx.clone());
    let ctx_value = super::loader::action_context_userdata(&cfg.lua, action_ctx.clone())
        .map_err(|err| super::render::mlua_error_to_config(cfg, &err))?;
    Ok((action_ctx, ctx_value))
}

/// Drain the queued outputs from a completed handler context.
fn collect_handler_result(action_ctx: &ActionCtx) -> HandlerResult {
    HandlerResult {
        effects: action_ctx.take_effects(),
        nav: action_ctx.take_nav(),
        stay: action_ctx.stay(),
    }
}

/// Execute a handler closure and collect its queued effects.
pub fn execute_handler(
    cfg: &DynamicConfig,
    handler: &HandlerRef,
    ctx: &ModeCtx,
) -> Result<HandlerResult, Error> {
    let (action_ctx, ctx_value) = prepare_handler_call(cfg, ctx)?;
    handler
        .func
        .call::<()>(ctx_value)
        .map_err(|err| super::render::mlua_error_to_config(cfg, &err))?;
    Ok(collect_handler_result(&action_ctx))
}

/// Execute a selector handler closure with `(ctx, item, query)` arguments.
pub fn execute_selector_handler(
    cfg: &DynamicConfig,
    handler: &HandlerRef,
    ctx: &ModeCtx,
    item: &SelectorItem,
    query: &str,
) -> Result<HandlerResult, Error> {
    let (action_ctx, ctx_value) = prepare_handler_call(cfg, ctx)?;

    let item_table = cfg
        .lua
        .create_table()
        .map_err(|err| super::render::mlua_error_to_config(cfg, &err))?;
    item_table
        .set("label", item.label.clone())
        .and_then(|()| item_table.set("sublabel", item.sublabel.clone()))
        .and_then(|()| item_table.set("data", item.data.clone()))
        .map_err(|err| super::render::mlua_error_to_config(cfg, &err))?;

    handler
        .func
        .call::<()>((ctx_value, Value::Table(item_table), query.to_string()))
        .map_err(|err| super::render::mlua_error_to_config(cfg, &err))?;

    Ok(collect_handler_result(&action_ctx))
}
