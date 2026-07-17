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

// ===== RAW NOTIFICATION STYLE =====

/// Raw notification style with all optional fields for merging
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RawNotifyStyle {
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

impl RawNotifyStyle {
    /// Convert to final NotifyStyle with defaults applied
    pub fn into_notify_style(self, defaults: crate::NotifyWindowStyle) -> crate::NotifyWindowStyle {
        crate::NotifyWindowStyle {
            bg: color_or(self.bg.as_deref(), defaults.bg),
            title_fg: color_or(self.title_fg.as_deref(), defaults.title_fg),
            body_fg: color_or(self.body_fg.as_deref(), defaults.body_fg),
            title_font_size: self.title_font_size.unwrap_or(defaults.title_font_size),
            title_font_weight: self.title_font_weight.unwrap_or(defaults.title_font_weight),
            body_font_size: self.body_font_size.unwrap_or(defaults.body_font_size),
            body_font_weight: self.body_font_weight.unwrap_or(defaults.body_font_weight),
            icon: self.icon.or(defaults.icon),
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
    pub width: Option<f32>,
    /// Screen side to stack notifications.
    #[serde(default)]
    pub pos: Option<NotifyPos>,
    /// Window opacity (0.0–1.0).
    #[serde(default)]
    pub opacity: Option<f32>,
    /// Auto-dismiss timeout in seconds.
    #[serde(default)]
    pub timeout: Option<f32>,
    /// Ring buffer length for notifications.
    #[serde(default)]
    pub buffer: Option<usize>,
    /// Corner radius (px).
    #[serde(default)]
    pub radius: Option<f32>,
    /// Style overrides for info notifications.
    #[serde(default)]
    pub info: Option<RawNotifyStyle>,
    /// Style overrides for warning notifications.
    #[serde(default)]
    pub warn: Option<RawNotifyStyle>,
    /// Style overrides for error notifications.
    #[serde(default)]
    pub error: Option<RawNotifyStyle>,
    /// Style overrides for success notifications.
    #[serde(default)]
    pub success: Option<RawNotifyStyle>,
}

impl RawNotify {
    /// Validate user-provided notification settings before style resolution.
    fn validate(&self) -> Result<(), String> {
        if let Some(timeout) = self.timeout.as_ref() {
            validate_notify_timeout(*timeout)?;
        }
        Ok(())
    }

    /// Convert to final Notify using the provided base as defaults
    pub fn into_notify_over(self, base: &Notify) -> Notify {
        let defaults = base.clone();
        Notify {
            width: self.width.unwrap_or(defaults.width),
            pos: self.pos.unwrap_or(defaults.pos),
            opacity: self.opacity.unwrap_or(defaults.opacity),
            timeout: self.timeout.unwrap_or(defaults.timeout),
            buffer: self.buffer.unwrap_or(defaults.buffer),
            radius: self.radius.unwrap_or(defaults.radius),
            theme: crate::NotifyTheme {
                info: self.info.map_or_else(
                    || defaults.theme.info.clone(),
                    |style| style.into_notify_style(defaults.theme.info.clone()),
                ),
                warn: self.warn.map_or_else(
                    || defaults.theme.warn.clone(),
                    |style| style.into_notify_style(defaults.theme.warn.clone()),
                ),
                error: self.error.map_or_else(
                    || defaults.theme.error.clone(),
                    |style| style.into_notify_style(defaults.theme.error.clone()),
                ),
                success: self.success.map_or_else(
                    || defaults.theme.success.clone(),
                    |style| style.into_notify_style(defaults.theme.success.clone()),
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
    pub min_duration_ms: Option<u64>,
    /// Full-width row background color.
    #[serde(default)]
    pub bg: Option<String>,
    /// Row description foreground color.
    #[serde(default)]
    pub title_fg: Option<String>,
    /// Non-modifier key foreground color.
    #[serde(default)]
    pub key_fg: Option<String>,
    /// Non-modifier key background color.
    #[serde(default)]
    pub key_bg: Option<String>,
    /// Modifier key foreground color.
    #[serde(default)]
    pub mod_fg: Option<String>,
    /// Modifier key background color.
    #[serde(default)]
    pub mod_bg: Option<String>,
    /// Submenu tag foreground color.
    #[serde(default)]
    pub tag_fg: Option<String>,
}

impl RawHudPressed {
    /// Convert to a resolved pressed-row style over the supplied base.
    fn into_pressed_over(
        self,
        base: &hotki_protocol::HudPressedStyle,
    ) -> hotki_protocol::HudPressedStyle {
        hotki_protocol::HudPressedStyle {
            min_duration_ms: self.min_duration_ms.unwrap_or(base.min_duration_ms),
            bg: color_or(self.bg.as_deref(), base.bg),
            title_fg: color_or(self.title_fg.as_deref(), base.title_fg),
            key_fg: color_or(self.key_fg.as_deref(), base.key_fg),
            key_bg: color_or(self.key_bg.as_deref(), base.key_bg),
            mod_fg: color_or(self.mod_fg.as_deref(), base.mod_fg),
            mod_bg: color_or(self.mod_bg.as_deref(), base.mod_bg),
            tag_fg: color_or(self.tag_fg.as_deref(), base.tag_fg),
        }
    }

    /// Validate the pressed-row duration.
    fn validate(&self) -> Result<(), String> {
        if self
            .min_duration_ms
            .as_ref()
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
    pub mode: Option<crate::Mode>,
    /// HUD position on screen.
    #[serde(default)]
    pub pos: Option<Pos>,
    /// HUD offset from `pos` in pixels.
    #[serde(default)]
    pub offset: Option<Offset>,
    /// Title font size in points.
    #[serde(default)]
    pub font_size: Option<f32>,
    /// Title font weight.
    #[serde(default)]
    pub title_font_weight: Option<FontWeight>,
    /// Key glyph font size in points.
    #[serde(default)]
    pub key_font_size: Option<f32>,
    /// Key glyph font weight.
    #[serde(default)]
    pub key_font_weight: Option<FontWeight>,
    /// Tag font size in points.
    #[serde(default)]
    pub tag_font_size: Option<f32>,
    /// Tag font weight.
    #[serde(default)]
    pub tag_font_weight: Option<FontWeight>,
    /// Title foreground color name or hex string.
    #[serde(default)]
    pub title_fg: Option<String>,
    /// HUD background color.
    #[serde(default)]
    pub bg: Option<String>,
    /// Key foreground color.
    #[serde(default)]
    pub key_fg: Option<String>,
    /// Key background color.
    #[serde(default)]
    pub key_bg: Option<String>,
    /// Modifier key foreground color.
    #[serde(default)]
    pub mod_fg: Option<String>,
    /// Modifier key font weight.
    #[serde(default)]
    pub mod_font_weight: Option<FontWeight>,
    /// Modifier key background color.
    #[serde(default)]
    pub mod_bg: Option<String>,
    /// Tag foreground color.
    #[serde(default)]
    pub tag_fg: Option<String>,
    /// HUD opacity (0.0–1.0).
    #[serde(default)]
    pub opacity: Option<f32>,
    /// Key corner radius (px).
    #[serde(default)]
    pub key_radius: Option<f32>,
    /// Horizontal key padding (px).
    #[serde(default)]
    pub key_pad_x: Option<f32>,
    /// Vertical key padding (px).
    #[serde(default)]
    pub key_pad_y: Option<f32>,
    /// HUD corner radius (px).
    #[serde(default)]
    pub radius: Option<f32>,
    /// Tag submenu glyph.
    #[serde(default)]
    pub tag_submenu: Option<String>,
    /// Pressed stay-binding row styling.
    #[serde(default)]
    pub pressed: Option<RawHudPressed>,
}

// ===== RAW SELECTOR STYLE =====

/// Raw selector style with all optional fields for merging.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RawSelector {
    /// Background color name or hex string.
    #[serde(default)]
    pub bg: Option<String>,
    /// Input background color name or hex string.
    #[serde(default)]
    pub input_bg: Option<String>,
    /// Item background color name or hex string.
    #[serde(default)]
    pub item_bg: Option<String>,
    /// Selected item background color name or hex string.
    #[serde(default)]
    pub item_selected_bg: Option<String>,
    /// Matched character foreground color name or hex string.
    #[serde(default)]
    pub match_fg: Option<String>,
    /// Border color name or hex string.
    #[serde(default)]
    pub border: Option<String>,
    /// Shadow color name or hex string.
    #[serde(default)]
    pub shadow: Option<String>,
}

impl RawSelector {
    /// Convert to final Selector using the provided base as defaults.
    pub fn into_selector_over(self, base: &Selector) -> Selector {
        let parse_color = |s: Option<String>, fallback: (u8, u8, u8)| {
            s.as_deref().and_then(parse_rgb).unwrap_or(fallback)
        };

        Selector {
            bg: parse_color(self.bg, base.bg),
            input_bg: parse_color(self.input_bg, base.input_bg),
            item_bg: parse_color(self.item_bg, base.item_bg),
            item_selected_bg: parse_color(self.item_selected_bg, base.item_selected_bg),
            match_fg: parse_color(self.match_fg, base.match_fg),
            border: parse_color(self.border, base.border),
            shadow: parse_color(self.shadow, base.shadow),
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
                color_or(self.$field.as_ref().map(|s| s.as_str()), defaults.$field)
            };
        }

        let mode = self.mode.unwrap_or(defaults.mode);
        let font_size = self.font_size.unwrap_or(defaults.font_size);
        let title_fg = color_field!(title_fg);
        let bg = color_field!(bg);
        let key_fg = color_field!(key_fg);
        let key_bg = color_field!(key_bg);
        let mod_fg = color_field!(mod_fg);
        let mod_bg = color_field!(mod_bg);
        let tag_fg = color_field!(tag_fg);

        Hud {
            mode,
            pos: self.pos.unwrap_or(defaults.pos),
            offset: self.offset.unwrap_or(defaults.offset),
            font_size,
            title_font_weight: self.title_font_weight.unwrap_or(defaults.title_font_weight),
            // key_font_size and tag_font_size default to font_size if not specified
            key_font_size: self.key_font_size.unwrap_or(font_size),
            key_font_weight: self.key_font_weight.unwrap_or(defaults.key_font_weight),
            tag_font_size: self.tag_font_size.unwrap_or(font_size),
            tag_font_weight: self.tag_font_weight.unwrap_or(defaults.tag_font_weight),
            title_fg,
            bg,
            key_fg,
            key_bg,
            mod_fg,
            mod_font_weight: self.mod_font_weight.unwrap_or(defaults.mod_font_weight),
            mod_bg,
            tag_fg,
            opacity: self.opacity.unwrap_or(defaults.opacity),
            key_radius: self.key_radius.unwrap_or(defaults.key_radius),
            key_pad_x: self.key_pad_x.unwrap_or(defaults.key_pad_x),
            key_pad_y: self.key_pad_y.unwrap_or(defaults.key_pad_y),
            radius: self.radius.unwrap_or(defaults.radius),
            tag_submenu: self
                .tag_submenu
                .unwrap_or_else(|| defaults.tag_submenu.clone()),
            pressed: self.pressed.map_or_else(
                || defaults.pressed.clone(),
                |pressed| pressed.into_pressed_over(&defaults.pressed),
            ),
        }
    }

    /// Validate nested HUD style values.
    fn validate(&self) -> Result<(), String> {
        if let Some(pressed) = self.pressed.as_ref() {
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
    pub hud: Option<RawHud>,
    /// Optional notification style overrides.
    #[serde(default)]
    pub notify: Option<RawNotify>,
    /// Optional selector style overrides.
    #[serde(default)]
    pub selector: Option<RawSelector>,
}

impl RawStyle {
    /// Validate raw style values that cannot be represented safely at runtime.
    pub fn validate(&self) -> Result<(), String> {
        if let Some(hud) = self.hud.as_ref() {
            hud.validate()?;
        }
        if let Some(notify) = self.notify.as_ref() {
            notify.validate()?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{HUD_PRESSED_MAX_DURATION_MS, RawHud, RawNotify, RawStyle};

    #[test]
    fn raw_struct_accepts_optional_field_values() {
        let hud: RawHud = serde_json::from_value(json!({ "radius": 8.0 })).unwrap();
        assert_eq!(hud.radius.as_ref().copied(), Some(8.0));
    }

    #[test]
    fn raw_style_rejects_non_finite_notify_timeout() {
        let style = RawStyle {
            notify: Some(RawNotify {
                timeout: Some(f32::INFINITY),
                ..RawNotify::default()
            }),
            ..RawStyle::default()
        };

        assert!(style.validate().is_err());
    }

    #[test]
    fn raw_style_rejects_out_of_range_notify_timeout() {
        let style: RawStyle =
            serde_json::from_value(json!({ "notify": { "timeout": 0.0 } })).unwrap();

        assert!(style.validate().is_err());
    }

    #[test]
    fn raw_style_accepts_zero_pressed_duration() {
        let style: RawStyle =
            serde_json::from_value(json!({ "hud": { "pressed": { "min_duration_ms": 0 } } }))
                .unwrap();

        assert_eq!(style.validate(), Ok(()));
    }

    #[test]
    fn raw_style_rejects_excessive_pressed_duration() {
        let style: RawStyle = serde_json::from_value(json!({
            "hud": {
                "pressed": {
                    "min_duration_ms": HUD_PRESSED_MAX_DURATION_MS + 1,
                },
            },
        }))
        .unwrap();

        assert!(style.validate().is_err());
    }
}
