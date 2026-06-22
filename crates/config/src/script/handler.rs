use ruau::vm::{CallOptions, ScriptError};

use super::{ActionCtx, DynamicConfig, HandlerRef, ModeCtx, NavRequest, SelectorItem, diagnostics};
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
    cfg: &mut DynamicConfig,
    handler: &HandlerRef,
    ctx: &ModeCtx,
) -> Result<HandlerResult, Error> {
    let action_ctx = ActionCtx::new(ctx.clone());
    let mut script_error = None;
    let path = cfg.path.clone();
    let sources = cfg.sources.clone();

    cfg.vm
        .step_with(
            &CallOptions::new().limits(DynamicConfig::entry_limits()),
            |scope| {
                let ctx_value =
                    super::host_userdata::action_context_userdata(scope, action_ctx.clone())?;
                let handler = scope.fetch_function(&handler.func)?;
                let result: Result<(), ScriptError<'_>> =
                    scope.call_protected(handler, ctx_value)?;
                if let Err(err) = result {
                    script_error = Some(diagnostics::config_script_error(
                        path.as_deref(),
                        &sources,
                        scope,
                        &err,
                    ));
                }
                Ok(())
            },
        )
        .map_err(|err| diagnostics::config_runtime_error(cfg.path.clone(), &err))?;

    if let Some(err) = script_error {
        return Err(err);
    }
    Ok(collect_handler_result(&action_ctx))
}

/// Execute a selector handler closure with `(ctx, item, query)` arguments.
pub fn execute_selector_handler(
    cfg: &mut DynamicConfig,
    handler: &HandlerRef,
    ctx: &ModeCtx,
    item: &SelectorItem,
    query: &str,
) -> Result<HandlerResult, Error> {
    let action_ctx = ActionCtx::new(ctx.clone());
    let mut script_error = None;
    let path = cfg.path.clone();
    let sources = cfg.sources.clone();
    let query = query.to_string();

    cfg.vm
        .step_with(
            &CallOptions::new().limits(DynamicConfig::entry_limits()),
            |scope| {
                let ctx_value =
                    super::host_userdata::action_context_userdata(scope, action_ctx.clone())?;
                let item_table = scope.create_table()?;
                item_table.set(scope, "label", item.label.clone())?;
                item_table.set(scope, "sublabel", item.sublabel.clone())?;
                item_table.set(scope, "data", item.data.fetch(scope)?)?;

                let handler = scope.fetch_function(&handler.func)?;
                let result: Result<(), ScriptError<'_>> =
                    scope.call_protected(handler, (ctx_value, item_table, query.clone()))?;
                if let Err(err) = result {
                    script_error = Some(diagnostics::config_script_error(
                        path.as_deref(),
                        &sources,
                        scope,
                        &err,
                    ));
                }
                Ok(())
            },
        )
        .map_err(|err| diagnostics::config_runtime_error(cfg.path.clone(), &err))?;

    if let Some(err) = script_error {
        return Err(err);
    }
    Ok(collect_handler_result(&action_ctx))
}
