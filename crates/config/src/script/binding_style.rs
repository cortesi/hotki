use serde::Deserialize;

use super::BindingStyle;
use crate::raw::{Maybe, RawHud, RawStyle};

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
/// Per-binding HUD style overrides parsed from Luau options tables.
pub struct BindingStyleSpec {
    /// Optional hidden flag override.
    hidden: Option<bool>,
    /// Optional key foreground color.
    key_fg: Option<String>,
    /// Optional key background color.
    key_bg: Option<String>,
    /// Optional modifier foreground color.
    mod_fg: Option<String>,
    /// Optional modifier background color.
    mod_bg: Option<String>,
    /// Optional tag foreground color.
    tag_fg: Option<String>,
}

impl BindingStyleSpec {
    /// Convert the parsed Luau record into the engine-facing binding style.
    pub(crate) fn into_binding_style(self) -> BindingStyle {
        let overlay = if self.key_fg.is_some()
            || self.key_bg.is_some()
            || self.mod_fg.is_some()
            || self.mod_bg.is_some()
            || self.tag_fg.is_some()
        {
            Some(RawStyle {
                hud: Maybe::Value(RawHud {
                    key_fg: self.key_fg.map_or(Maybe::Unit(()), Maybe::Value),
                    key_bg: self.key_bg.map_or(Maybe::Unit(()), Maybe::Value),
                    mod_fg: self.mod_fg.map_or(Maybe::Unit(()), Maybe::Value),
                    mod_bg: self.mod_bg.map_or(Maybe::Unit(()), Maybe::Value),
                    tag_fg: self.tag_fg.map_or(Maybe::Unit(()), Maybe::Value),
                    ..RawHud::default()
                }),
                ..RawStyle::default()
            })
        } else {
            None
        };

        BindingStyle {
            hidden: self.hidden.unwrap_or(false),
            overlay,
        }
    }
}
