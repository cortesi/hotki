//! Selector binding configuration types.

use oxau::{
    embed::{Function, RuntimeError, Scope, ScopedValue, StashedClosure, StashedValue, Table},
    session::CallOptions,
};

use super::{DynamicConfig, HandlerRef, ModeCtx, diagnostics};

/// Opaque selector item payload stashed in the config VM.
#[derive(Debug, Clone, Default)]
pub struct SelectorData {
    /// Stashed Luau value retained by the config VM.
    value: Option<StashedValue>,
}

impl SelectorData {
    /// Create selector data from a stashed VM value.
    pub(crate) fn new(value: StashedValue) -> Self {
        Self { value: Some(value) }
    }

    /// Fetch the payload value inside the current VM scope.
    pub(crate) fn fetch<'s>(&self, scope: &Scope<'s>) -> Result<ScopedValue<'s>, RuntimeError> {
        let value = self
            .value
            .as_ref()
            .ok_or_else(|| RuntimeError::runtime("selector item has no script data"))?;
        scope.fetch_value(value)
    }
}

/// Parse a selector item from a Luau string or table value.
pub fn parse_selector_item<'s>(
    scope: &Scope<'s>,
    index: usize,
    value: ScopedValue<'s>,
) -> Result<SelectorItem, RuntimeError> {
    match value {
        ScopedValue::String(text) => {
            let label = String::from_utf8(scope.string_bytes(text)?)
                .map_err(|_| RuntimeError::runtime("selector label must be UTF-8"))?;
            let data = SelectorData::new(scope.stash_value(ScopedValue::String(text))?);
            Ok(SelectorItem {
                label,
                sublabel: None,
                data,
            })
        }
        ScopedValue::Table(table) => selector_item_from_table(scope, index, table),
        other => Err(RuntimeError::runtime(format!(
            "selector.items: element {} must be a string or table, got {}",
            index,
            other.type_name()
        ))),
    }
}

/// Parse a selector item from a Luau table value.
fn selector_item_from_table<'s>(
    scope: &Scope<'s>,
    index: usize,
    table: Table<'s>,
) -> Result<SelectorItem, RuntimeError> {
    let label_value = table.get(scope, "label").map_err(|_| {
        RuntimeError::runtime(format!(
            "selector.items: element {} missing required field 'label'",
            index
        ))
    })?;
    let label = String::from_utf8(scope.string_bytes(label_value)?)
        .map_err(|_| RuntimeError::runtime("selector label must be UTF-8"))?;

    let sublabel: Option<String> = table.get(scope, "sublabel").map_err(|_| {
        RuntimeError::runtime(format!(
            "selector.items: element {} field 'sublabel' must be a string",
            index
        ))
    })?;

    let data_value = match table.get::<_, ScopedValue<'_>>(scope, "data") {
        Ok(ScopedValue::Nil) | Err(_) => ScopedValue::String(label_value),
        Ok(value) => value,
    };
    let data = SelectorData::new(scope.stash_value(data_value)?);

    Ok(SelectorItem {
        label,
        sublabel,
        data,
    })
}

/// Parse selector items from a Luau array-like table.
pub fn parse_selector_items<'s>(
    scope: &Scope<'s>,
    value: ScopedValue<'s>,
) -> Result<Vec<SelectorItem>, RuntimeError> {
    let ScopedValue::Table(table) = value else {
        return Err(RuntimeError::runtime(format!(
            "selector.items must be an array or provider function, got {}",
            value.type_name()
        )));
    };

    let len = usize::try_from(table.len(scope)?)
        .map_err(|_| RuntimeError::runtime("selector.items length does not fit usize"))?;
    let mut items = Vec::with_capacity(len);
    for index in 1..=len {
        let value = table.get(scope, index as f64)?;
        items.push(parse_selector_item(scope, index, value)?);
    }
    Ok(items)
}

/// Parse a selector configuration record from Luau.
pub fn parse_selector_config<'s>(
    scope: &Scope<'s>,
    value: ScopedValue<'s>,
) -> Result<SelectorConfig, RuntimeError> {
    let ScopedValue::Table(table) = value else {
        return Err(RuntimeError::runtime("action.selector expects a table"));
    };

    let items_value: ScopedValue<'_> = table
        .get(scope, "items")
        .map_err(|_| RuntimeError::runtime("selector: missing required field 'items'"))?;
    let items = match items_value {
        ScopedValue::Function(func) => SelectorItems::Provider(scope.stash_function(func)?),
        other => SelectorItems::Static(parse_selector_items(scope, other)?),
    };

    let on_select = table
        .get(scope, "on_select")
        .map_err(|_| RuntimeError::runtime("selector: missing required field 'on_select'"))?;
    let on_select = HandlerRef::from_function(scope, on_select)?;
    let on_cancel = table
        .get::<_, Option<Function<'_>>>(scope, "on_cancel")?
        .map(|func| HandlerRef::from_function(scope, func))
        .transpose()?;

    Ok(SelectorConfig {
        title: table
            .get::<_, Option<String>>(scope, "title")?
            .unwrap_or_else(|| "Select".to_string()),
        placeholder: table
            .get::<_, Option<String>>(scope, "placeholder")?
            .unwrap_or_default(),
        items,
        on_select,
        on_cancel,
        max_visible: table
            .get::<_, Option<usize>>(scope, "max_visible")?
            .unwrap_or(10),
    })
}

/// A single selectable option in an interactive selector.
#[derive(Debug, Clone)]
pub struct SelectorItem {
    /// Primary text displayed in the list.
    pub label: String,
    /// Optional secondary text shown below the label.
    pub sublabel: Option<String>,
    /// Arbitrary auxiliary data passed to the callback on selection.
    pub data: SelectorData,
}

/// Item source for a selector.
#[derive(Debug, Clone)]
pub enum SelectorItems {
    /// Static item list.
    Static(Vec<SelectorItem>),
    /// Lazy item provider evaluated when the selector is opened.
    Provider(StashedClosure),
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
    pub on_select: HandlerRef,
    /// Optional callback invoked on cancel.
    pub on_cancel: Option<HandlerRef>,
    /// Maximum number of items to display at once.
    pub max_visible: usize,
}

impl SelectorConfig {
    /// Resolve items for this selector, evaluating a provider function when needed.
    pub fn resolve_items(
        &self,
        cfg: &mut DynamicConfig,
        ctx: &ModeCtx,
    ) -> Result<Vec<SelectorItem>, crate::Error> {
        match &self.items {
            SelectorItems::Static(items) => Ok(items.clone()),
            SelectorItems::Provider(provider) => {
                let mut items = None;
                let mut script_error = None;
                let path = cfg.path.clone();
                let sources = cfg.sources.clone();
                cfg.vm
                    .step_with(
                        CallOptions::new().limits(DynamicConfig::entry_limits()),
                        |scope| {
                            let provider = scope.fetch_function(provider)?;
                            let ctx_value =
                                super::host_userdata::mode_context_userdata(scope, ctx.clone())?;
                            let result = scope.call_protected(provider, ctx_value)?;
                            match result {
                                Ok(value) => items = Some(parse_selector_items(scope, value)?),
                                Err(err) => {
                                    script_error = Some(diagnostics::config_script_error(
                                        path.as_deref(),
                                        &sources,
                                        scope,
                                        &err,
                                    ));
                                }
                            }
                            Ok(())
                        },
                    )
                    .map_err(|err| diagnostics::config_runtime_error(cfg.path.clone(), &err))?;

                if let Some(err) = script_error {
                    return Err(err);
                }
                items.ok_or_else(|| crate::Error::Validation {
                    path: cfg.path.clone(),
                    line: None,
                    col: None,
                    message: "selector provider returned no items".to_string(),
                    excerpt: None,
                })
            }
        }
    }
}
