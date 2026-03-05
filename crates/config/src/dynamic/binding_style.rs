use std::path::PathBuf;

use rhai::{Dynamic, EvalAltResult, Map, Position, serde::from_dynamic};

use super::validation::boxed_validation_error;
use crate::{
    Error,
    raw::{Maybe, RawHud, RawStyle},
};

#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(deny_unknown_fields)]
/// Binding-level style overrides accepted by the DSL and style closures.
pub struct RawBindingStyle {
    #[serde(default)]
    /// Whether to hide the binding from the HUD.
    hidden: Option<bool>,
    #[serde(default)]
    /// Override key foreground color.
    key_fg: Option<String>,
    #[serde(default)]
    /// Override key background color.
    key_bg: Option<String>,
    #[serde(default)]
    /// Override modifier foreground color.
    mod_fg: Option<String>,
    #[serde(default)]
    /// Override modifier background color.
    mod_bg: Option<String>,
    #[serde(default)]
    /// Override submenu tag color.
    tag_fg: Option<String>,
}

#[derive(Debug, Clone)]
/// Resolved binding style overlay used by the renderer and DSL.
pub struct ParsedBindingStyle {
    /// Whether the binding should be hidden from the HUD.
    pub(crate) hidden: bool,
    /// Optional style overlay to apply to the base HUD style.
    pub(crate) overlay: Option<RawStyle>,
}

/// Parse a binding-style map in a DSL context with a Rhai call position.
pub fn parse_binding_style_map(
    map: Map,
    pos: Position,
) -> Result<ParsedBindingStyle, Box<EvalAltResult>> {
    parse_binding_style_dynamic(&Dynamic::from_map(map))
        .map_err(|err| boxed_validation_error(format!("invalid binding style map: {}", err), pos))
}

/// Parse a binding-style dynamic value in a config/render context.
pub fn parse_binding_style_value(
    value: &Dynamic,
    path: Option<&PathBuf>,
) -> Result<ParsedBindingStyle, Error> {
    parse_binding_style_dynamic(value).map_err(|err| Error::Validation {
        path: path.cloned(),
        line: None,
        col: None,
        message: format!("invalid binding style map: {}", err),
        excerpt: None,
    })
}

/// Deserialize the common binding-style schema from a dynamic Rhai value.
fn parse_binding_style_dynamic(value: &Dynamic) -> Result<ParsedBindingStyle, String> {
    let style: RawBindingStyle = from_dynamic(value).map_err(|err| err.to_string())?;
    Ok(ParsedBindingStyle {
        hidden: style.hidden.unwrap_or(false),
        overlay: raw_overlay(&style),
    })
}

/// Convert parsed binding-style fields into a raw HUD overlay.
fn raw_overlay(style: &RawBindingStyle) -> Option<RawStyle> {
    let mut hud = RawHud::default();
    if let Some(value) = &style.key_fg {
        hud.key_fg = Maybe::Value(value.clone());
    }
    if let Some(value) = &style.key_bg {
        hud.key_bg = Maybe::Value(value.clone());
    }
    if let Some(value) = &style.mod_fg {
        hud.mod_fg = Maybe::Value(value.clone());
    }
    if let Some(value) = &style.mod_bg {
        hud.mod_bg = Maybe::Value(value.clone());
    }
    if let Some(value) = &style.tag_fg {
        hud.tag_fg = Maybe::Value(value.clone());
    }

    if hud.key_fg.as_option().is_none()
        && hud.key_bg.as_option().is_none()
        && hud.mod_fg.as_option().is_none()
        && hud.mod_bg.as_option().is_none()
        && hud.tag_fg.as_option().is_none()
    {
        return None;
    }

    Some(RawStyle {
        hud: Maybe::Value(hud),
        notify: Maybe::Unit(()),
        selector: Maybe::Unit(()),
    })
}
