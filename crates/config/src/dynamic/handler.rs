use rhai::{Dynamic, Map};

use super::{ActionCtx, DynamicConfig, HandlerRef, ModeCtx, NavRequest, SelectorItem};
use crate::Error;

/// Result of executing a handler closure.
#[derive(Debug)]
pub struct HandlerResult {
    /// Side effects queued by the handler (actions, notifications, navigation).
    pub effects: Vec<super::Effect>,
    /// Optional navigation request emitted by the handler.
    pub nav: Option<NavRequest>,
    /// True when the handler requested to suppress auto-exit behavior.
    pub stay: bool,
}

/// Execute a handler closure and collect its queued effects.
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

/// Execute a selector handler closure with `(ctx, item, query)` arguments.
pub fn execute_selector_handler(
    cfg: &DynamicConfig,
    handler: &HandlerRef,
    ctx: &ModeCtx,
    item: &SelectorItem,
    query: &str,
) -> Result<HandlerResult, Error> {
    let action_ctx = ActionCtx::new(
        ctx.app.clone(),
        ctx.title.clone(),
        ctx.pid,
        ctx.hud,
        ctx.depth,
    );

    let mut m = Map::new();
    m.insert("label".into(), Dynamic::from(item.label.clone()));
    m.insert(
        "sublabel".into(),
        item.sublabel
            .clone()
            .map(Dynamic::from)
            .unwrap_or(Dynamic::UNIT),
    );
    m.insert("data".into(), item.data.clone());
    let item_map = Dynamic::from_map(m);

    handler
        .func
        .call::<Dynamic>(
            &cfg.engine,
            &cfg.ast,
            (action_ctx.clone(), item_map, query.to_string()),
        )
        .map(|_| ())
        .map_err(|err| super::render::rhai_error_to_config(cfg, &err))?;

    Ok(HandlerResult {
        effects: action_ctx.take_effects(),
        nav: action_ctx.take_nav(),
        stay: action_ctx.stay(),
    })
}
