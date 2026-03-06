use rhai::{Dynamic, FnPtr, Map, NativeCallContext};

use super::super::{HandlerRef, SelectorConfig, SelectorItems};

/// Parse selector items from either a static array or a lazy provider closure.
fn selector_items_from_dynamic(
    ctx: &NativeCallContext,
    value: Dynamic,
) -> Result<SelectorItems, Box<rhai::EvalAltResult>> {
    if let Some(func) = value.clone().try_cast::<FnPtr>() {
        return Ok(SelectorItems::Provider(func));
    }

    let arr = value.into_array().map_err(|_| {
        super::boxed_validation_error(
            "selector.items must be an array or a closure".to_string(),
            ctx.call_position(),
        )
    })?;

    let items = super::super::selector::parse_selector_items(arr)
        .map_err(|msg| super::boxed_validation_error(msg, ctx.call_position()))?;
    Ok(SelectorItems::Static(items))
}

/// Parse a selector callback handler from a Rhai closure or `action.run(...)`.
fn selector_handler_required(
    ctx: &NativeCallContext,
    field: &str,
    value: Dynamic,
) -> Result<HandlerRef, Box<rhai::EvalAltResult>> {
    if let Some(handler) = value.clone().try_cast::<HandlerRef>() {
        return Ok(handler);
    }
    if let Some(func) = value.try_cast::<FnPtr>() {
        return Ok(HandlerRef { func });
    }
    Err(super::boxed_validation_error(
        format!("selector.{} must be a closure (or action.run(...))", field),
        ctx.call_position(),
    ))
}

/// Parse an optional selector callback handler from a Rhai value.
fn selector_handler_optional(
    ctx: &NativeCallContext,
    field: &str,
    value: Dynamic,
) -> Result<Option<HandlerRef>, Box<rhai::EvalAltResult>> {
    if value.is_unit() {
        return Ok(None);
    }
    selector_handler_required(ctx, field, value).map(Some)
}

/// Parse a selector config map into a `SelectorConfig`, validating the schema.
pub(super) fn selector_config_from_map(
    ctx: &NativeCallContext,
    map: Map,
) -> Result<SelectorConfig, Box<rhai::EvalAltResult>> {
    let mut title = String::new();
    let mut placeholder = String::new();
    let mut items = None;
    let mut on_select = None;
    let mut on_cancel = None;
    let mut max_visible = 10_usize;

    for (key, value) in map {
        match key.as_str() {
            "title" => {
                title = value
                    .into_immutable_string()
                    .map_err(|_| {
                        super::boxed_validation_error(
                            "selector.title must be a string".to_string(),
                            ctx.call_position(),
                        )
                    })?
                    .to_string();
            }
            "placeholder" => {
                placeholder = value
                    .into_immutable_string()
                    .map_err(|_| {
                        super::boxed_validation_error(
                            "selector.placeholder must be a string".to_string(),
                            ctx.call_position(),
                        )
                    })?
                    .to_string();
            }
            "items" => {
                items = Some(selector_items_from_dynamic(ctx, value)?);
            }
            "on_select" => {
                on_select = Some(selector_handler_required(ctx, "on_select", value)?);
            }
            "on_cancel" => {
                on_cancel = selector_handler_optional(ctx, "on_cancel", value)?;
            }
            "max_visible" => {
                let visible_count: i64 = value.try_cast::<i64>().ok_or_else(|| {
                    super::boxed_validation_error(
                        "selector.max_visible must be an integer".to_string(),
                        ctx.call_position(),
                    )
                })?;
                if visible_count <= 0 {
                    return Err(super::boxed_validation_error(
                        "selector.max_visible must be > 0".to_string(),
                        ctx.call_position(),
                    ));
                }
                max_visible = visible_count as usize;
            }
            _ => {
                return Err(super::boxed_validation_error(
                    format!("selector: unknown field '{}'", key),
                    ctx.call_position(),
                ));
            }
        }
    }

    let items = items.ok_or_else(|| {
        super::boxed_validation_error(
            "selector: missing required field 'items'".to_string(),
            ctx.call_position(),
        )
    })?;
    let on_select = on_select.ok_or_else(|| {
        super::boxed_validation_error(
            "selector: missing required field 'on_select'".to_string(),
            ctx.call_position(),
        )
    })?;

    Ok(SelectorConfig {
        title,
        placeholder,
        items,
        on_select,
        on_cancel,
        max_visible,
    })
}
