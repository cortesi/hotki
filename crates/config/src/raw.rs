//! Raw configuration structures mirroring the serialized user input.

use serde::{Deserialize, Serialize};

use super::{
    Hud, Notify, Selector, parse_rgb,
    types::{FontWeight, NotifyPos, Offset, Pos},
};

/// Smallest accepted notification auto-dismiss timeout.
const NOTIFY_TIMEOUT_MIN_SECS: f32 = 0.1;
/// Largest accepted notification auto-dismiss timeout.
const NOTIFY_TIMEOUT_MAX_SECS: f32 = 3600.0;
/// Largest accepted minimum duration for HUD press feedback.
const HUD_PRESSED_MAX_DURATION_MS: u64 = 2000;

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
    /// Validate user-provided notification settings before style resolution.
    fn validate(&self) -> Result<(), String> {
        if let Some(timeout) = self.timeout.as_option() {
            validate_notify_timeout(*timeout)?;
        }
        Ok(())
    }

    /// Convert to final Notify using the provided base as defaults
    pub fn into_notify_over(self, base: &Notify) -> Notify {
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
}

/// Validate the finite notification timeout range accepted by the UI.
fn validate_notify_timeout(timeout: f32) -> Result<(), String> {
    if !timeout.is_finite() {
        return Err("notify.timeout must be finite".to_string());
    }
    if !(NOTIFY_TIMEOUT_MIN_SECS..=NOTIFY_TIMEOUT_MAX_SECS).contains(&timeout) {
        return Err(format!(
            "notify.timeout must be between {NOTIFY_TIMEOUT_MIN_SECS} and \
             {NOTIFY_TIMEOUT_MAX_SECS} seconds"
        ));
    }
    Ok(())
}

// ===== RAW HUD CONFIG =====

/// Raw pressed-row HUD style with all optional fields for merging.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RawHudPressed {
    /// Minimum visible press duration in milliseconds.
    #[serde(default)]
    pub min_duration_ms: Maybe<u64>,
    /// Full-width row background color.
    #[serde(default)]
    pub bg: Maybe<String>,
    /// Row description foreground color.
    #[serde(default)]
    pub title_fg: Maybe<String>,
    /// Non-modifier key foreground color.
    #[serde(default)]
    pub key_fg: Maybe<String>,
    /// Non-modifier key background color.
    #[serde(default)]
    pub key_bg: Maybe<String>,
    /// Modifier key foreground color.
    #[serde(default)]
    pub mod_fg: Maybe<String>,
    /// Modifier key background color.
    #[serde(default)]
    pub mod_bg: Maybe<String>,
    /// Submenu tag foreground color.
    #[serde(default)]
    pub tag_fg: Maybe<String>,
}

impl RawHudPressed {
    /// Convert to a resolved pressed-row style over the supplied base.
    fn into_pressed_over(
        self,
        base: &hotki_protocol::HudPressedStyle,
    ) -> hotki_protocol::HudPressedStyle {
        hotki_protocol::HudPressedStyle {
            min_duration_ms: maybe_or(self.min_duration_ms, base.min_duration_ms),
            bg: color_or(self.bg.as_option().map(String::as_str), base.bg),
            title_fg: color_or(self.title_fg.as_option().map(String::as_str), base.title_fg),
            key_fg: color_or(self.key_fg.as_option().map(String::as_str), base.key_fg),
            key_bg: color_or(self.key_bg.as_option().map(String::as_str), base.key_bg),
            mod_fg: color_or(self.mod_fg.as_option().map(String::as_str), base.mod_fg),
            mod_bg: color_or(self.mod_bg.as_option().map(String::as_str), base.mod_bg),
            tag_fg: color_or(self.tag_fg.as_option().map(String::as_str), base.tag_fg),
        }
    }

    /// Validate the pressed-row duration.
    fn validate(&self) -> Result<(), String> {
        if self
            .min_duration_ms
            .as_option()
            .is_some_and(|duration| *duration > HUD_PRESSED_MAX_DURATION_MS)
        {
            return Err(format!(
                "hud.pressed.min_duration_ms must be between 0 and {HUD_PRESSED_MAX_DURATION_MS}"
            ));
        }
        Ok(())
    }
}

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
    /// Pressed stay-binding row styling.
    #[serde(default)]
    pub pressed: Maybe<RawHudPressed>,
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
    /// Convert to final Hud using the provided base as defaults
    pub fn into_hud_over(self, base: &Hud) -> Hud {
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
            pressed: apply_optional_overlay(
                self.pressed.into_option(),
                &defaults.pressed,
                RawHudPressed::into_pressed_over,
            ),
        }
    }

    /// Validate nested HUD style values.
    fn validate(&self) -> Result<(), String> {
        if let Some(pressed) = self.pressed.as_option() {
            pressed.validate()?;
        }
        Ok(())
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
    /// Validate raw style values that cannot be represented safely at runtime.
    pub fn validate(&self) -> Result<(), String> {
        if let Some(hud) = self.hud.as_option() {
            hud.validate()?;
        }
        if let Some(notify) = self.notify.as_option() {
            notify.validate()?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{HUD_PRESSED_MAX_DURATION_MS, Maybe, RawHud, RawHudPressed, RawNotify, RawStyle};

    #[test]
    fn raw_struct_accepts_bare_maybe_field_values() {
        let hud: RawHud = serde_json::from_value(json!({ "radius": 8.0 })).unwrap();
        assert_eq!(hud.radius.as_option().copied(), Some(8.0));
    }

    #[test]
    fn raw_style_rejects_non_finite_notify_timeout() {
        let style = RawStyle {
            notify: Maybe::Value(RawNotify {
                timeout: Maybe::Value(f32::INFINITY),
                ..RawNotify::default()
            }),
            ..RawStyle::default()
        };

        assert!(style.validate().is_err());
    }

    #[test]
    fn raw_style_rejects_out_of_range_notify_timeout() {
        let style = RawStyle {
            notify: Maybe::Value(RawNotify {
                timeout: Maybe::Value(0.0),
                ..RawNotify::default()
            }),
            ..RawStyle::default()
        };

        assert!(style.validate().is_err());
    }

    #[test]
    fn raw_style_accepts_zero_pressed_duration() {
        let style = RawStyle {
            hud: Maybe::Value(RawHud {
                pressed: Maybe::Value(RawHudPressed {
                    min_duration_ms: Maybe::Value(0),
                    ..RawHudPressed::default()
                }),
                ..RawHud::default()
            }),
            ..RawStyle::default()
        };

        assert_eq!(style.validate(), Ok(()));
    }

    #[test]
    fn raw_style_rejects_excessive_pressed_duration() {
        let style = RawStyle {
            hud: Maybe::Value(RawHud {
                pressed: Maybe::Value(RawHudPressed {
                    min_duration_ms: Maybe::Value(HUD_PRESSED_MAX_DURATION_MS + 1),
                    ..RawHudPressed::default()
                }),
                ..RawHud::default()
            }),
            ..RawStyle::default()
        };

        assert!(style.validate().is_err());
    }
}
