//! Style and HUD configuration types.

use serde::{Deserialize, Serialize};

use crate::{FontWeight, Mode, Offset, Pos, defaults, notify::Notify, parse_rgb, raw};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// Visual theme configuration grouping HUD and notification settings.
pub struct Style {
    /// HUD configuration section.
    #[serde(default)]
    pub hud: Hud,

    /// Notification configuration section.
    #[serde(default)]
    pub notify: Notify,
}

impl Style {
    /// Overlay raw style overrides onto this base style using current values as defaults.
    pub(crate) fn overlay_raw(mut self, overrides: &raw::RawStyle) -> Self {
        if let Some(h) = overrides.hud.as_option() {
            self.hud = h.clone().into_hud_over(&self.hud);
        }
        if let Some(n) = overrides.notify.as_option() {
            self.notify = n.clone().into_notify_over(&self.notify);
        }
        self
    }

    /// Apply multiple raw overlays left-to-right.
    pub(crate) fn overlay_all_raw(mut self, overlays: &[raw::RawStyle]) -> Self {
        for ov in overlays {
            self = self.overlay_raw(ov);
        }
        self
    }

    /// Convert a fully resolved style into a raw style overlay with all fields populated.
    pub(crate) fn to_raw(&self) -> raw::RawStyle {
        raw::RawStyle {
            hud: raw::Maybe::Value(self.hud.to_raw_hud()),
            notify: raw::Maybe::Value(self.notify.to_raw_notify()),
        }
    }
}

/// Convert an RGB tuple into a canonical `#rrggbb` string.
fn rgb_to_hex((r, g, b): (u8, u8, u8)) -> String {
    format!("#{:02x}{:02x}{:02x}", r, g, b)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// HUD configuration section.
pub struct Hud {
    /// Display mode selection for the HUD.
    #[serde(default)]
    pub mode: Mode,
    /// Screen anchor position for the HUD window.
    #[serde(default)]
    pub pos: Pos,
    /// Pixel offset added to the anchored position. Positive `x` moves right; positive `y` moves up.
    #[serde(default)]
    pub offset: Offset,
    /// Base font size for descriptions and general HUD text.
    #[serde(default = "defaults::default_font_size")]
    pub font_size: f32,
    /// Font weight for title/description text.
    #[serde(default)]
    pub title_font_weight: FontWeight,
    /// Font size for key tokens inside their rounded boxes. Defaults to `font_size`.
    pub key_font_size: f32,
    /// Font weight for non-modifier key tokens.
    #[serde(default)]
    pub key_font_weight: FontWeight,
    /// Font size for the tag indicator shown for sub-modes. Defaults to `font_size`.
    pub tag_font_size: f32,
    /// Font weight for the sub-mode tag indicator.
    #[serde(default)]
    pub tag_font_weight: FontWeight,
    /// Foreground color for title/description text (parsed RGB).
    pub title_fg: (u8, u8, u8),
    /// HUD background fill color (parsed RGB).
    pub bg: (u8, u8, u8),
    /// Foreground color for non-modifier key tokens (parsed RGB).
    pub key_fg: (u8, u8, u8),
    /// Background color for non-modifier key tokens (parsed RGB).
    pub key_bg: (u8, u8, u8),
    /// Foreground color for modifier key tokens (parsed RGB).
    pub mod_fg: (u8, u8, u8),
    /// Font weight for modifier key tokens.
    #[serde(default)]
    pub mod_font_weight: FontWeight,
    /// Background color for modifier key tokens (parsed RGB).
    pub mod_bg: (u8, u8, u8),
    /// Foreground color for the sub-mode tag indicator (parsed RGB).
    pub tag_fg: (u8, u8, u8),
    /// Window opacity in the range [0.0, 1.0]. `1.0` is fully opaque.
    #[serde(default = "defaults::default_opacity")]
    pub opacity: f32,
    /// Corner radius for key boxes.
    #[serde(default = "defaults::default_key_radius")]
    pub key_radius: f32,
    /// Horizontal padding inside key boxes.
    #[serde(default = "defaults::default_key_pad_x")]
    pub key_pad_x: f32,
    /// Vertical padding inside key boxes.
    #[serde(default = "defaults::default_key_pad_y")]
    pub key_pad_y: f32,
    /// Corner radius for the HUD window itself.
    #[serde(default = "defaults::default_radius")]
    pub radius: f32,
    /// Text tag shown for sub-modes at the end of rows.
    #[serde(default = "defaults::default_tag_submenu")]
    pub tag_submenu: String,
}

impl Default for Hud {
    fn default() -> Self {
        let parse_or = |s: &str| parse_rgb(s).unwrap_or((255, 255, 255));
        let fs = defaults::HUD_FONT_SIZE;
        Self {
            mode: Mode::Hud,
            pos: defaults::HUD_POS,
            offset: defaults::HUD_OFFSET,
            font_size: fs,
            title_font_weight: FontWeight::Regular,
            key_font_size: fs,
            key_font_weight: FontWeight::Regular,
            tag_font_size: fs,
            tag_font_weight: FontWeight::Regular,
            title_fg: parse_or(defaults::HUD_TITLE_FG),
            bg: parse_or(defaults::HUD_BG),
            key_fg: parse_or(defaults::HUD_KEY_FG),
            key_bg: parse_or(defaults::HUD_KEY_BG),
            mod_fg: parse_or(defaults::HUD_MOD_FG),
            mod_font_weight: FontWeight::Regular,
            mod_bg: parse_or(defaults::HUD_MOD_BG),
            tag_fg: parse_or(defaults::HUD_TAG_FG),
            opacity: defaults::HUD_OPACITY,
            key_radius: defaults::KEY_RADIUS,
            key_pad_x: defaults::KEY_PAD_X,
            key_pad_y: defaults::KEY_PAD_Y,
            radius: defaults::HUD_RADIUS,
            tag_submenu: defaults::TAG_SUBMENU.to_string(),
        }
    }
}

// Parsed HUD colors are stored directly on Hud; palette helper removed.

impl Hud {
    /// Convert a concrete HUD style into a raw style overlay with all fields populated.
    fn to_raw_hud(&self) -> raw::RawHud {
        raw::RawHud {
            mode: raw::Maybe::Value(self.mode),
            pos: raw::Maybe::Value(self.pos),
            offset: raw::Maybe::Value(self.offset),
            font_size: raw::Maybe::Value(self.font_size),
            title_font_weight: raw::Maybe::Value(self.title_font_weight),
            key_font_size: raw::Maybe::Value(self.key_font_size),
            key_font_weight: raw::Maybe::Value(self.key_font_weight),
            tag_font_size: raw::Maybe::Value(self.tag_font_size),
            tag_font_weight: raw::Maybe::Value(self.tag_font_weight),
            title_fg: raw::Maybe::Value(rgb_to_hex(self.title_fg)),
            bg: raw::Maybe::Value(rgb_to_hex(self.bg)),
            key_fg: raw::Maybe::Value(rgb_to_hex(self.key_fg)),
            key_bg: raw::Maybe::Value(rgb_to_hex(self.key_bg)),
            mod_fg: raw::Maybe::Value(rgb_to_hex(self.mod_fg)),
            mod_font_weight: raw::Maybe::Value(self.mod_font_weight),
            mod_bg: raw::Maybe::Value(rgb_to_hex(self.mod_bg)),
            tag_fg: raw::Maybe::Value(rgb_to_hex(self.tag_fg)),
            opacity: raw::Maybe::Value(self.opacity),
            key_radius: raw::Maybe::Value(self.key_radius),
            key_pad_x: raw::Maybe::Value(self.key_pad_x),
            key_pad_y: raw::Maybe::Value(self.key_pad_y),
            radius: raw::Maybe::Value(self.radius),
            tag_submenu: raw::Maybe::Value(self.tag_submenu.clone()),
        }
    }
}
