//! Selector binding configuration types.

use rhai::{Dynamic, EvalAltResult, FnPtr};

use super::{DynamicConfig, HandlerRef, ModeCtx};

/// Parse a selector item from a Rhai string or map value.
pub fn parse_selector_item(index: usize, value: Dynamic) -> Result<SelectorItem, String> {
    if let Ok(label) = value.clone().into_immutable_string() {
        let label = label.to_string();
        return Ok(SelectorItem {
            label: label.clone(),
            sublabel: None,
            data: Dynamic::from(label),
        });
    }

    let map = value
        .try_cast::<rhai::Map>()
        .ok_or_else(|| format!("selector.items: element {} must be a string or map", index))?;

    let label = map.get("label").cloned().ok_or_else(|| {
        format!(
            "selector.items: element {} missing required field 'label'",
            index
        )
    })?;
    let label = label.into_immutable_string().map_err(|_| {
        format!(
            "selector.items: element {} field 'label' must be a string",
            index
        )
    })?;
    let label = label.to_string();

    let sublabel = match map.get("sublabel").cloned() {
        None => None,
        Some(v) if v.is_unit() => None,
        Some(v) => Some(
            v.into_immutable_string()
                .map_err(|_| {
                    format!(
                        "selector.items: element {} field 'sublabel' must be a string",
                        index
                    )
                })?
                .to_string(),
        ),
    };

    let data = match map.get("data").cloned() {
        Some(v) if !v.is_unit() => v,
        _ => Dynamic::from(label.clone()),
    };

    Ok(SelectorItem {
        label,
        sublabel,
        data,
    })
}

/// Parse a selector items array from Rhai into a `Vec<SelectorItem>`.
pub fn parse_selector_items(items: rhai::Array) -> Result<Vec<SelectorItem>, String> {
    let mut parsed = Vec::with_capacity(items.len());
    for (i, v) in items.into_iter().enumerate() {
        parsed.push(parse_selector_item(i, v)?);
    }
    Ok(parsed)
}

/// A single selectable option in an interactive selector.
#[derive(Debug, Clone)]
pub struct SelectorItem {
    /// Primary text displayed in the list.
    pub label: String,
    /// Optional secondary text (smaller, dimmed) shown below/beside label.
    pub sublabel: Option<String>,
    /// Arbitrary auxiliary data passed to the callback on selection.
    ///
    /// Stored as Rhai `Dynamic` for flexibility.
    pub data: Dynamic,
}

/// Item source for a selector.
#[derive(Debug, Clone)]
pub enum SelectorItems {
    /// Static item list.
    Static(Vec<SelectorItem>),
    /// Lazy item provider evaluated when the selector is opened.
    ///
    /// Signature: `|ctx| -> Array`
    Provider(FnPtr),
}

/// Configuration for an interactive selector instance.
#[derive(Debug, Clone)]
pub struct SelectorConfig {
    /// Window title shown in the selector header.
    pub title: String,
    /// Placeholder text shown in the empty input field.
    pub placeholder: String,
    /// Item source for the selector.
    pub items: SelectorItems,
    /// Callback invoked on selection.
    ///
    /// Signature: `|ctx, item, query| { ... }`
    pub on_select: HandlerRef,
    /// Optional callback invoked on cancel (Escape); defaults to no-op.
    pub on_cancel: Option<HandlerRef>,
    /// Maximum number of items to display at once.
    pub max_visible: usize,
}

impl SelectorConfig {
    /// Resolve items for this selector, evaluating a provider function when needed.
    pub fn resolve_items(
        &self,
        cfg: &DynamicConfig,
        ctx: &ModeCtx,
    ) -> Result<Vec<SelectorItem>, crate::Error> {
        match &self.items {
            SelectorItems::Static(items) => Ok(items.clone()),
            SelectorItems::Provider(provider) => {
                let arr = provider
                    .call::<rhai::Array>(&cfg.engine, &cfg.ast, (ctx.clone(),))
                    .or_else(|err| match err.as_ref() {
                        EvalAltResult::ErrorFunctionNotFound(_, _) => {
                            provider.call::<rhai::Array>(&cfg.engine, &cfg.ast, ())
                        }
                        _ => Err(err),
                    })
                    .map_err(|err| super::render::rhai_error_to_config(cfg, &err))?;

                parse_selector_items(arr).map_err(|message| crate::Error::Validation {
                    path: cfg.path.clone(),
                    line: None,
                    col: None,
                    message,
                    excerpt: None,
                })
            }
        }
    }
}
