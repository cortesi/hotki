//! Raw configuration structures mirroring the serialized user input.

use serde::{Deserialize, Serialize};

use super::{
    Hud, Notify, parse_rgb,
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
    pub fn into_notify_style(self, defaults: RawNotifyWindowStyle) -> RawNotifyWindowStyle {
        RawNotifyWindowStyle {
            bg: self.bg.into_option().or(defaults.bg),
            title_fg: self.title_fg.into_option().or(defaults.title_fg),
            body_fg: self.body_fg.into_option().or(defaults.body_fg),
            title_font_size: self
                .title_font_size
                .into_option()
                .or(defaults.title_font_size),
            title_font_weight: self
                .title_font_weight
                .into_option()
                .or(defaults.title_font_weight),
            body_font_size: self
                .body_font_size
                .into_option()
                .or(defaults.body_font_size),
            body_font_weight: self
                .body_font_weight
                .into_option()
                .or(defaults.body_font_weight),
            icon: self.icon.into_option().or(defaults.icon),
        }
    }

    /// Merge another notification style on top of this one.
    pub(crate) fn merge(&self, other: &Self) -> Self {
        Self {
            bg: merge_maybe(&self.bg, &other.bg),
            title_fg: merge_maybe(&self.title_fg, &other.title_fg),
            body_fg: merge_maybe(&self.body_fg, &other.body_fg),
            title_font_size: merge_maybe(&self.title_font_size, &other.title_font_size),
            title_font_weight: merge_maybe(&self.title_font_weight, &other.title_font_weight),
            body_font_size: merge_maybe(&self.body_font_size, &other.body_font_size),
            body_font_weight: merge_maybe(&self.body_font_weight, &other.body_font_weight),
            icon: merge_maybe(&self.icon, &other.icon),
        }
    }
}

/// Raw notification window styling read from configuration (string colors, optional sizes/weights).
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct RawNotifyWindowStyle {
    /// Background color name or hex string.
    #[serde(default)]
    pub bg: Option<String>,
    /// Title foreground color name or hex string.
    #[serde(default)]
    pub title_fg: Option<String>,
    /// Body foreground color name or hex string.
    #[serde(default)]
    pub body_fg: Option<String>,
    /// Title font size in points.
    #[serde(default)]
    pub title_font_size: Option<f32>,
    /// Title font weight.
    #[serde(default)]
    pub title_font_weight: Option<FontWeight>,
    /// Body font size in points.
    #[serde(default)]
    pub body_font_size: Option<f32>,
    /// Body font weight.
    #[serde(default)]
    pub body_font_weight: Option<FontWeight>,
    /// Optional leading icon string.
    #[serde(default)]
    pub icon: Option<String>,
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
        macro_rules! or_field {
            ($field:ident) => {
                self.$field.into_option().unwrap_or(defaults.$field)
            };
        }
        Notify {
            width: or_field!(width),
            pos: or_field!(pos),
            opacity: or_field!(opacity),
            timeout: or_field!(timeout),
            buffer: or_field!(buffer),
            radius: or_field!(radius),
            info: self
                .info
                .into_option()
                .map(|s| s.into_notify_style(defaults.info.clone()))
                .unwrap_or(defaults.info),
            warn: self
                .warn
                .into_option()
                .map(|s| s.into_notify_style(defaults.warn.clone()))
                .unwrap_or(defaults.warn),
            error: self
                .error
                .into_option()
                .map(|s| s.into_notify_style(defaults.error.clone()))
                .unwrap_or(defaults.error),
            success: self
                .success
                .into_option()
                .map(|s| s.into_notify_style(defaults.success.clone()))
                .unwrap_or(defaults.success),
        }
    }

    /// Convert to final Notify using the provided base as defaults
    pub fn into_notify_over(self, base: &Notify) -> Notify {
        self.apply_over(base)
    }

    /// Merge another notification overlay on top of this one.
    pub(crate) fn merge(&self, other: &Self) -> Self {
        Self {
            width: merge_maybe(&self.width, &other.width),
            pos: merge_maybe(&self.pos, &other.pos),
            opacity: merge_maybe(&self.opacity, &other.opacity),
            timeout: merge_maybe(&self.timeout, &other.timeout),
            buffer: merge_maybe(&self.buffer, &other.buffer),
            radius: merge_maybe(&self.radius, &other.radius),
            info: merge_maybe_nested(&self.info, &other.info, RawNotifyStyle::merge),
            warn: merge_maybe_nested(&self.warn, &other.warn, RawNotifyStyle::merge),
            error: merge_maybe_nested(&self.error, &other.error, RawNotifyStyle::merge),
            success: merge_maybe_nested(&self.success, &other.success, RawNotifyStyle::merge),
        }
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
        macro_rules! or_field {
            ($field:ident) => {
                self.$field.into_option().unwrap_or(defaults.$field)
            };
        }
        macro_rules! color_field {
            ($field:ident) => {
                color_or(self.$field.as_option().map(|s| s.as_str()), defaults.$field)
            };
        }

        let mode = or_field!(mode);
        let font_size = or_field!(font_size);
        let title_fg = color_field!(title_fg);
        let bg = color_field!(bg);
        let key_fg = color_field!(key_fg);
        let key_bg = color_field!(key_bg);
        let mod_fg = color_field!(mod_fg);
        let mod_bg = color_field!(mod_bg);
        let tag_fg = color_field!(tag_fg);

        Hud {
            mode,
            pos: or_field!(pos),
            offset: or_field!(offset),
            font_size,
            title_font_weight: or_field!(title_font_weight),
            // key_font_size and tag_font_size default to font_size if not specified
            key_font_size: self.key_font_size.into_option().unwrap_or(font_size),
            key_font_weight: or_field!(key_font_weight),
            tag_font_size: self.tag_font_size.into_option().unwrap_or(font_size),
            tag_font_weight: or_field!(tag_font_weight),
            title_fg,
            bg,
            key_fg,
            key_bg,
            mod_fg,
            mod_font_weight: or_field!(mod_font_weight),
            mod_bg,
            tag_fg,
            opacity: or_field!(opacity),
            key_radius: or_field!(key_radius),
            key_pad_x: or_field!(key_pad_x),
            key_pad_y: or_field!(key_pad_y),
            radius: or_field!(radius),
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
        Self {
            mode: merge_maybe(&self.mode, &other.mode),
            pos: merge_maybe(&self.pos, &other.pos),
            offset: merge_maybe(&self.offset, &other.offset),
            font_size: merge_maybe(&self.font_size, &other.font_size),
            title_font_weight: merge_maybe(&self.title_font_weight, &other.title_font_weight),
            key_font_size: merge_maybe(&self.key_font_size, &other.key_font_size),
            key_font_weight: merge_maybe(&self.key_font_weight, &other.key_font_weight),
            tag_font_size: merge_maybe(&self.tag_font_size, &other.tag_font_size),
            tag_font_weight: merge_maybe(&self.tag_font_weight, &other.tag_font_weight),
            title_fg: merge_maybe(&self.title_fg, &other.title_fg),
            bg: merge_maybe(&self.bg, &other.bg),
            key_fg: merge_maybe(&self.key_fg, &other.key_fg),
            key_bg: merge_maybe(&self.key_bg, &other.key_bg),
            mod_fg: merge_maybe(&self.mod_fg, &other.mod_fg),
            mod_font_weight: merge_maybe(&self.mod_font_weight, &other.mod_font_weight),
            mod_bg: merge_maybe(&self.mod_bg, &other.mod_bg),
            tag_fg: merge_maybe(&self.tag_fg, &other.tag_fg),
            opacity: merge_maybe(&self.opacity, &other.opacity),
            key_radius: merge_maybe(&self.key_radius, &other.key_radius),
            key_pad_x: merge_maybe(&self.key_pad_x, &other.key_pad_x),
            key_pad_y: merge_maybe(&self.key_pad_y, &other.key_pad_y),
            radius: merge_maybe(&self.radius, &other.radius),
            tag_submenu: merge_maybe(&self.tag_submenu, &other.tag_submenu),
        }
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
}

impl RawStyle {
    /// Merge another style overlay on top of this one.
    pub(crate) fn merge(&self, other: &Self) -> Self {
        Self {
            hud: merge_maybe_nested(&self.hud, &other.hud, RawHud::merge),
            notify: merge_maybe_nested(&self.notify, &other.notify, RawNotify::merge),
        }
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
