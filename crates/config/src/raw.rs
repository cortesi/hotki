//! Raw configuration structures mirroring the serialized user input.

use serde::{Deserialize, Serialize};

use super::{
    Hud, Notify, Selector, parse_rgb,
    types::{FontWeight, NotifyPos, Offset, Pos},
};

// ===== FIELD WRAPPERS FOR OPTIONAL VALUES =====

/// Generic optional wrapper used when deserializing user config values.
///
/// Purpose
/// - Treats an omitted field or explicit unit `()` as not provided (None).
/// - Accepts a plain value `T` as provided (Some(T)).
/// - Accepts an `Option<T>` for completeness and pass‑through.
///
/// This wrapper lets top‑level and nested style sections be succinct while still
/// allowing explicit nulling where supported.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(untagged)]
pub enum Maybe<T> {
    /// Explicit unit `()` treated as not provided.
    Unit(()),
    /// Plain value provided.
    Value(T),
    /// Optional value, passes through as-is.
    Opt(Option<T>),
}

impl<T> Default for Maybe<T> {
    fn default() -> Self {
        Self::Unit(())
    }
}

impl<T> Maybe<T> {
    /// Convert to an owned `Option<T>` according to wrapper semantics.
    pub fn into_option(self) -> Option<T> {
        match self {
            Self::Unit(()) => None,
            Self::Value(v) => Some(v),
            Self::Opt(o) => o,
        }
    }
    /// Borrow as `Option<&T>` according to wrapper semantics.
    pub fn as_option(&self) -> Option<&T> {
        match self {
            Self::Unit(()) => None,
            Self::Value(v) => Some(v),
            Self::Opt(Some(v)) => Some(v),
            Self::Opt(None) => None,
        }
    }
}

/// Extract the overlay value or fall back to `default`.
fn maybe_or<T>(value: Maybe<T>, default: T) -> T {
    value.into_option().unwrap_or(default)
}

/// Merge two `Maybe<T>` values, preferring the overlay when it is present.
fn merge_maybe<T: Clone>(base: &Maybe<T>, overlay: &Maybe<T>) -> Maybe<T> {
    match overlay.as_option() {
        Some(_) => overlay.clone(),
        None => base.clone(),
    }
}

/// Merge two nested `Maybe<T>` values, merging inner values when both are present.
fn merge_maybe_nested<T: Clone>(
    base: &Maybe<T>,
    overlay: &Maybe<T>,
    merge: impl FnOnce(&T, &T) -> T,
) -> Maybe<T> {
    match (base.as_option(), overlay.as_option()) {
        (Some(b), Some(o)) => Maybe::Value(merge(b, o)),
        (Some(_), None) => base.clone(),
        (None, Some(_)) => overlay.clone(),
        (None, None) => base.clone(),
    }
}

/// Apply an optional overlay section to a resolved protocol-facing style value.
pub fn apply_optional_overlay<T, U: Clone>(
    overlay: Option<T>,
    base: &U,
    apply: impl FnOnce(T, &U) -> U,
) -> U {
    overlay
        .map(|overlay| apply(overlay, base))
        .unwrap_or_else(|| base.clone())
}

/// Merge overlay structs field-by-field, with optional nested merge functions.
macro_rules! merge_overlay {
    ($base:expr, $overlay:expr; nested[$($nested:ident => $merge:path),* $(,)?]) => {
        Self {
            $(
                $nested: merge_maybe_nested(&$base.$nested, &$overlay.$nested, $merge),
            )*
        }
    };
    ($base:expr, $overlay:expr; flat[$($field:ident),* $(,)?] $(; nested[$($nested:ident => $merge:path),* $(,)?])?) => {
        Self {
            $(
                $field: merge_maybe(&$base.$field, &$overlay.$field),
            )*
            $(
                $(
                    $nested: merge_maybe_nested(&$base.$nested, &$overlay.$nested, $merge),
                )*
            )?
        }
    };
}

// ===== RAW NOTIFICATION STYLE =====

/// Raw notification style with all optional fields for merging
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RawNotifyStyle {
    /// Background color name or hex string.
    #[serde(default)]
    pub bg: Maybe<String>,
    /// Title foreground color name or hex string.
    #[serde(default)]
    pub title_fg: Maybe<String>,
    /// Body foreground color name or hex string.
    #[serde(default)]
    pub body_fg: Maybe<String>,
    /// Title font size in points.
    #[serde(default)]
    pub title_font_size: Maybe<f32>,
    /// Title font weight.
    #[serde(default)]
    pub title_font_weight: Maybe<FontWeight>,
    /// Body font size in points.
    #[serde(default)]
    pub body_font_size: Maybe<f32>,
    /// Body font weight.
    #[serde(default)]
    pub body_font_weight: Maybe<FontWeight>,
    /// Optional leading icon string.
    #[serde(default)]
    pub icon: Maybe<String>,
}

impl RawNotifyStyle {
    /// Convert to final NotifyStyle with defaults applied
    pub fn into_notify_style(self, defaults: crate::NotifyWindowStyle) -> crate::NotifyWindowStyle {
        crate::NotifyWindowStyle {
            bg: color_or(self.bg.as_option().map(String::as_str), defaults.bg),
            title_fg: color_or(
                self.title_fg.as_option().map(String::as_str),
                defaults.title_fg,
            ),
            body_fg: color_or(
                self.body_fg.as_option().map(String::as_str),
                defaults.body_fg,
            ),
            title_font_size: maybe_or(self.title_font_size, defaults.title_font_size),
            title_font_weight: maybe_or(self.title_font_weight, defaults.title_font_weight),
            body_font_size: maybe_or(self.body_font_size, defaults.body_font_size),
            body_font_weight: maybe_or(self.body_font_weight, defaults.body_font_weight),
            icon: self.icon.into_option().or(defaults.icon),
        }
    }

    /// Merge another notification style on top of this one.
    pub(crate) fn merge(&self, other: &Self) -> Self {
        merge_overlay!(
            self,
            other;
            flat[
                bg,
                title_fg,
                body_fg,
                title_font_size,
                title_font_weight,
                body_font_size,
                body_font_weight,
                icon,
            ]
        )
    }
}

// ===== RAW NOTIFICATION CONFIG =====

/// Raw notification config with all optional fields for conversion
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RawNotify {
    /// Width of notification window (px).
    #[serde(default)]
    pub width: Maybe<f32>,
    /// Screen side to stack notifications.
    #[serde(default)]
    pub pos: Maybe<NotifyPos>,
    /// Window opacity (0.0–1.0).
    #[serde(default)]
    pub opacity: Maybe<f32>,
    /// Auto-dismiss timeout in seconds.
    #[serde(default)]
    pub timeout: Maybe<f32>,
    /// Ring buffer length for notifications.
    #[serde(default)]
    pub buffer: Maybe<usize>,
    /// Corner radius (px).
    #[serde(default)]
    pub radius: Maybe<f32>,
    /// Style overrides for info notifications.
    #[serde(default)]
    pub info: Maybe<RawNotifyStyle>,
    /// Style overrides for warning notifications.
    #[serde(default)]
    pub warn: Maybe<RawNotifyStyle>,
    /// Style overrides for error notifications.
    #[serde(default)]
    pub error: Maybe<RawNotifyStyle>,
    /// Style overrides for success notifications.
    #[serde(default)]
    pub success: Maybe<RawNotifyStyle>,
}

impl RawNotify {
    /// Internal helper: apply overrides over a base Notify
    fn apply_over(self, base: &Notify) -> Notify {
        let defaults = base.clone();
        Notify {
            width: maybe_or(self.width, defaults.width),
            pos: maybe_or(self.pos, defaults.pos),
            opacity: maybe_or(self.opacity, defaults.opacity),
            timeout: maybe_or(self.timeout, defaults.timeout),
            buffer: maybe_or(self.buffer, defaults.buffer),
            radius: maybe_or(self.radius, defaults.radius),
            theme: crate::NotifyTheme {
                info: apply_optional_overlay(
                    self.info.into_option(),
                    &defaults.theme.info,
                    |style, base| style.into_notify_style(base.clone()),
                ),
                warn: apply_optional_overlay(
                    self.warn.into_option(),
                    &defaults.theme.warn,
                    |style, base| style.into_notify_style(base.clone()),
                ),
                error: apply_optional_overlay(
                    self.error.into_option(),
                    &defaults.theme.error,
                    |style, base| style.into_notify_style(base.clone()),
                ),
                success: apply_optional_overlay(
                    self.success.into_option(),
                    &defaults.theme.success,
                    |style, base| style.into_notify_style(base.clone()),
                ),
            },
        }
    }

    /// Convert to final Notify using the provided base as defaults
    pub fn into_notify_over(self, base: &Notify) -> Notify {
        self.apply_over(base)
    }

    /// Merge another notification overlay on top of this one.
    pub(crate) fn merge(&self, other: &Self) -> Self {
        merge_overlay!(
            self,
            other;
            flat[width, pos, opacity, timeout, buffer, radius];
            nested[
                info => RawNotifyStyle::merge,
                warn => RawNotifyStyle::merge,
                error => RawNotifyStyle::merge,
                success => RawNotifyStyle::merge,
            ]
        )
    }
}

// ===== RAW HUD CONFIG =====

/// Raw HUD config with all optional fields for conversion
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RawHud {
    /// HUD display mode.
    #[serde(default)]
    pub mode: Maybe<crate::Mode>,
    /// HUD position on screen.
    #[serde(default)]
    pub pos: Maybe<Pos>,
    /// HUD offset from `pos` in pixels.
    #[serde(default)]
    pub offset: Maybe<Offset>,
    /// Title font size in points.
    #[serde(default)]
    pub font_size: Maybe<f32>,
    /// Title font weight.
    #[serde(default)]
    pub title_font_weight: Maybe<FontWeight>,
    /// Key glyph font size in points.
    #[serde(default)]
    pub key_font_size: Maybe<f32>,
    /// Key glyph font weight.
    #[serde(default)]
    pub key_font_weight: Maybe<FontWeight>,
    /// Tag font size in points.
    #[serde(default)]
    pub tag_font_size: Maybe<f32>,
    /// Tag font weight.
    #[serde(default)]
    pub tag_font_weight: Maybe<FontWeight>,
    /// Title foreground color name or hex string.
    #[serde(default)]
    pub title_fg: Maybe<String>,
    /// HUD background color.
    #[serde(default)]
    pub bg: Maybe<String>,
    /// Key foreground color.
    #[serde(default)]
    pub key_fg: Maybe<String>,
    /// Key background color.
    #[serde(default)]
    pub key_bg: Maybe<String>,
    /// Modifier key foreground color.
    #[serde(default)]
    pub mod_fg: Maybe<String>,
    /// Modifier key font weight.
    #[serde(default)]
    pub mod_font_weight: Maybe<FontWeight>,
    /// Modifier key background color.
    #[serde(default)]
    pub mod_bg: Maybe<String>,
    /// Tag foreground color.
    #[serde(default)]
    pub tag_fg: Maybe<String>,
    /// HUD opacity (0.0–1.0).
    #[serde(default)]
    pub opacity: Maybe<f32>,
    /// Key corner radius (px).
    #[serde(default)]
    pub key_radius: Maybe<f32>,
    /// Horizontal key padding (px).
    #[serde(default)]
    pub key_pad_x: Maybe<f32>,
    /// Vertical key padding (px).
    #[serde(default)]
    pub key_pad_y: Maybe<f32>,
    /// HUD corner radius (px).
    #[serde(default)]
    pub radius: Maybe<f32>,
    /// Tag submenu glyph.
    #[serde(default)]
    pub tag_submenu: Maybe<String>,
}

// ===== RAW SELECTOR STYLE =====

/// Raw selector style with all optional fields for merging.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RawSelector {
    /// Background color name or hex string.
    #[serde(default)]
    pub bg: Maybe<String>,
    /// Input background color name or hex string.
    #[serde(default)]
    pub input_bg: Maybe<String>,
    /// Item background color name or hex string.
    #[serde(default)]
    pub item_bg: Maybe<String>,
    /// Selected item background color name or hex string.
    #[serde(default)]
    pub item_selected_bg: Maybe<String>,
    /// Matched character foreground color name or hex string.
    #[serde(default)]
    pub match_fg: Maybe<String>,
    /// Border color name or hex string.
    #[serde(default)]
    pub border: Maybe<String>,
    /// Shadow color name or hex string.
    #[serde(default)]
    pub shadow: Maybe<String>,
}

impl RawSelector {
    /// Convert to final Selector using the provided base as defaults.
    pub fn into_selector_over(self, base: &Selector) -> Selector {
        let parse_color = |s: Option<String>, fallback: (u8, u8, u8)| {
            s.as_deref().and_then(parse_rgb).unwrap_or(fallback)
        };

        Selector {
            bg: parse_color(self.bg.into_option(), base.bg),
            input_bg: parse_color(self.input_bg.into_option(), base.input_bg),
            item_bg: parse_color(self.item_bg.into_option(), base.item_bg),
            item_selected_bg: parse_color(
                self.item_selected_bg.into_option(),
                base.item_selected_bg,
            ),
            match_fg: parse_color(self.match_fg.into_option(), base.match_fg),
            border: parse_color(self.border.into_option(), base.border),
            shadow: parse_color(self.shadow.into_option(), base.shadow),
        }
    }

    /// Merge another selector overlay on top of this one.
    pub(crate) fn merge(&self, other: &Self) -> Self {
        merge_overlay!(
            self,
            other;
            flat[bg, input_bg, item_bg, item_selected_bg, match_fg, border, shadow]
        )
    }
}

// Shared color fallback for HUD
/// Use `src` color if provided, otherwise fall back to `default`.
fn color_or(src: Option<&str>, default: (u8, u8, u8)) -> (u8, u8, u8) {
    match src {
        Some(s) => match parse_rgb(s) {
            Some(rgb) => rgb,
            None => {
                eprintln!("Warning: invalid color '{}', using default", s);
                default
            }
        },
        None => default,
    }
}

impl RawHud {
    /// Internal helper: apply overrides over a base Hud
    fn apply_over(self, base: &Hud) -> Hud {
        let defaults = base.clone();
        macro_rules! color_field {
            ($field:ident) => {
                color_or(self.$field.as_option().map(|s| s.as_str()), defaults.$field)
            };
        }

        let mode = maybe_or(self.mode, defaults.mode);
        let font_size = maybe_or(self.font_size, defaults.font_size);
        let title_fg = color_field!(title_fg);
        let bg = color_field!(bg);
        let key_fg = color_field!(key_fg);
        let key_bg = color_field!(key_bg);
        let mod_fg = color_field!(mod_fg);
        let mod_bg = color_field!(mod_bg);
        let tag_fg = color_field!(tag_fg);

        Hud {
            mode,
            pos: maybe_or(self.pos, defaults.pos),
            offset: maybe_or(self.offset, defaults.offset),
            font_size,
            title_font_weight: maybe_or(self.title_font_weight, defaults.title_font_weight),
            // key_font_size and tag_font_size default to font_size if not specified
            key_font_size: maybe_or(self.key_font_size, font_size),
            key_font_weight: maybe_or(self.key_font_weight, defaults.key_font_weight),
            tag_font_size: maybe_or(self.tag_font_size, font_size),
            tag_font_weight: maybe_or(self.tag_font_weight, defaults.tag_font_weight),
            title_fg,
            bg,
            key_fg,
            key_bg,
            mod_fg,
            mod_font_weight: maybe_or(self.mod_font_weight, defaults.mod_font_weight),
            mod_bg,
            tag_fg,
            opacity: maybe_or(self.opacity, defaults.opacity),
            key_radius: maybe_or(self.key_radius, defaults.key_radius),
            key_pad_x: maybe_or(self.key_pad_x, defaults.key_pad_x),
            key_pad_y: maybe_or(self.key_pad_y, defaults.key_pad_y),
            radius: maybe_or(self.radius, defaults.radius),
            tag_submenu: self
                .tag_submenu
                .into_option()
                .unwrap_or_else(|| defaults.tag_submenu.clone()),
        }
    }

    /// Convert to final Hud using the provided base as defaults
    pub fn into_hud_over(self, base: &Hud) -> Hud {
        self.apply_over(base)
    }

    /// Merge another HUD overlay on top of this one.
    pub(crate) fn merge(&self, other: &Self) -> Self {
        merge_overlay!(
            self,
            other;
            flat[
                mode,
                pos,
                offset,
                font_size,
                title_font_weight,
                key_font_size,
                key_font_weight,
                tag_font_size,
                tag_font_weight,
                title_fg,
                bg,
                key_fg,
                key_bg,
                mod_fg,
                mod_font_weight,
                mod_bg,
                tag_fg,
                opacity,
                key_radius,
                key_pad_x,
                key_pad_y,
                radius,
                tag_submenu,
            ]
        )
    }
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
/// Raw style overlay grouping HUD and notification sections.
pub struct RawStyle {
    /// Optional HUD style overrides.
    #[serde(default)]
    pub hud: Maybe<RawHud>,
    /// Optional notification style overrides.
    #[serde(default)]
    pub notify: Maybe<RawNotify>,
    /// Optional selector style overrides.
    #[serde(default)]
    pub selector: Maybe<RawSelector>,
}

impl RawStyle {
    /// Merge another style overlay on top of this one.
    pub(crate) fn merge(&self, other: &Self) -> Self {
        merge_overlay!(
            self,
            other;
            nested[
                hud => RawHud::merge,
                notify => RawNotify::merge,
                selector => RawSelector::merge,
            ]
        )
    }
}

#[cfg(test)]
mod tests {
    use rhai::{Dynamic, Map, serde::from_dynamic};

    use super::RawHud;

    #[test]
    fn raw_struct_accepts_bare_maybe_field_values() {
        let mut map = Map::new();
        map.insert("radius".into(), Dynamic::from(8.0_f64));
        let dyn_map = Dynamic::from_map(map);
        let hud: RawHud = from_dynamic(&dyn_map).unwrap();
        assert_eq!(hud.radius.as_option().copied(), Some(8.0));
    }
}
