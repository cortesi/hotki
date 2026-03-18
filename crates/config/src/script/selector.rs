//! Selector binding configuration types.

use mlua::{Function, String as LuaString, Table, Value};

use super::{DynamicConfig, HandlerRef, ModeCtx};

/// Parse a selector item from a Luau string or table value.
pub fn parse_selector_item(index: usize, value: Value) -> Result<SelectorItem, String> {
    match value {
        Value::String(text) => {
            let label = text.to_string_lossy();
            let data = Value::String(text);
            Ok(SelectorItem {
                label,
                sublabel: None,
                data,
            })
        }
        Value::Table(table) => selector_item_from_table(index, &table),
        other => Err(format!(
            "selector.items: element {} must be a string or table, got {}",
            index,
            other.type_name()
        )),
    }
}

/// Parse a selector item from a Luau table value.
fn selector_item_from_table(index: usize, table: &Table) -> Result<SelectorItem, String> {
    let label_value: LuaString = table.get("label").map_err(|_| {
        format!(
            "selector.items: element {} missing required field 'label'",
            index
        )
    })?;
    let label = label_value.to_string_lossy();

    let sublabel: Option<String> = table.get("sublabel").map_err(|_| {
        format!(
            "selector.items: element {} field 'sublabel' must be a string",
            index
        )
    })?;

    let data = match table.get::<Value>("data") {
        Ok(Value::Nil) | Err(_) => Value::String(label_value),
        Ok(value) => value,
    };

    Ok(SelectorItem {
        label,
        sublabel,
        data,
    })
}

/// Parse selector items from a Luau array-like table.
pub fn parse_selector_items(value: Value) -> Result<Vec<SelectorItem>, String> {
    let Value::Table(table) = value else {
        return Err(format!(
            "selector.items must be an array or provider function, got {}",
            value.type_name()
        ));
    };

    table
        .sequence_values::<Value>()
        .enumerate()
        .map(|(index, entry)| {
            entry
                .map_err(|err| err.to_string())
                .and_then(|value| parse_selector_item(index, value))
        })
        .collect()
}

/// A single selectable option in an interactive selector.
#[derive(Debug, Clone)]
pub struct SelectorItem {
    /// Primary text displayed in the list.
    pub label: String,
    /// Optional secondary text shown below the label.
    pub sublabel: Option<String>,
    /// Arbitrary auxiliary data passed to the callback on selection.
    pub data: Value,
}

/// Item source for a selector.
#[derive(Debug, Clone)]
pub enum SelectorItems {
    /// Static item list.
    Static(Vec<SelectorItem>),
    /// Lazy item provider evaluated when the selector is opened.
    Provider(Function),
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
        cfg: &DynamicConfig,
        ctx: &ModeCtx,
    ) -> Result<Vec<SelectorItem>, crate::Error> {
        match &self.items {
            SelectorItems::Static(items) => Ok(items.clone()),
            SelectorItems::Provider(provider) => {
                cfg.reset_execution_budget();
                let ctx_value = super::loader::mode_context_userdata(&cfg.lua, ctx.clone())
                    .map_err(|err| super::render::mlua_error_to_config(cfg, &err))?;
                let value = provider
                    .call::<Value>(ctx_value)
                    .map_err(|err| super::render::mlua_error_to_config(cfg, &err))?;
                parse_selector_items(value).map_err(|message| crate::Error::Validation {
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
