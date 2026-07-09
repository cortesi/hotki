use ruau::vm::{CallOptions, ScriptError};

use super::{
    ActionCtx, ActionRepeatPermission, DynamicConfig, HandlerRef, ModeCtx, SelectorItem,
    diagnostics,
};
use crate::Error;

/// Result of executing a handler closure.
#[derive(Debug)]
pub struct HandlerResult {
    /// Side effects queued by the handler.
    pub effects: Vec<super::Effect>,
    /// True when the handler requested to suppress auto-exit behavior.
    pub stay: bool,
}

/// Drain the queued outputs from a completed handler context.
fn collect_handler_result(action_ctx: &ActionCtx) -> HandlerResult {
    let (effects, stay) = action_ctx.finish();
    HandlerResult { effects, stay }
}

/// Execute a handler closure and collect its queued effects.
pub fn execute_handler(
    cfg: &mut DynamicConfig,
    handler: &HandlerRef,
    ctx: &ModeCtx,
) -> Result<HandlerResult, Error> {
    execute_handler_with_permission(cfg, handler, ctx, ActionRepeatPermission::HeldKey)
}

/// Execute a handler closure with an explicit repeat permission policy.
pub fn execute_handler_with_permission(
    cfg: &mut DynamicConfig,
    handler: &HandlerRef,
    ctx: &ModeCtx,
    repeat: ActionRepeatPermission,
) -> Result<HandlerResult, Error> {
    cfg.collect_entrypoint_garbage();
    let result = execute_handler_inner(cfg, handler, ctx, repeat);
    cfg.collect_entrypoint_garbage();
    result
}

/// Execute a handler closure without managing the retained VM heap boundary.
fn execute_handler_inner(
    cfg: &mut DynamicConfig,
    handler: &HandlerRef,
    ctx: &ModeCtx,
    repeat: ActionRepeatPermission,
) -> Result<HandlerResult, Error> {
    let action_ctx = ActionCtx::new(ctx.clone(), repeat);
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
        action_ctx.invalidate();
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
    cfg.collect_entrypoint_garbage();
    let result = execute_selector_handler_inner(cfg, handler, ctx, item, query);
    cfg.collect_entrypoint_garbage();
    result
}

/// Execute a selector handler without managing the retained VM heap boundary.
fn execute_selector_handler_inner(
    cfg: &mut DynamicConfig,
    handler: &HandlerRef,
    ctx: &ModeCtx,
    item: &SelectorItem,
    query: &str,
) -> Result<HandlerResult, Error> {
    let action_ctx = ActionCtx::new(ctx.clone(), ActionRepeatPermission::Keyless);
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
        action_ctx.invalidate();
        return Err(err);
    }
    Ok(collect_handler_result(&action_ctx))
}
