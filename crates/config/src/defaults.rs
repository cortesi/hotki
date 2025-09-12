// Defaults and constants for UI configuration

use crate::{NotifyPos, raw::RawNotifyWindowStyle};

pub(crate) const TAG_SUBMENU: &str = "\u{f035f}";
pub(crate) fn default_tag_submenu() -> String {
    TAG_SUBMENU.to_string()
}

// HUD defaults
pub(crate) const HUD_FONT_SIZE: f32 = 16.0;
pub(crate) const HUD_OPACITY: f32 = 1.0;
pub(crate) const HUD_TITLE_FG: &str = "white";
pub(crate) const HUD_BG: &str = "#202020";
pub(crate) const HUD_KEY_FG: &str = "white";
pub(crate) const HUD_KEY_BG: &str = "#303030";
pub(crate) const HUD_MOD_FG: &str = "white";
pub(crate) const HUD_MOD_BG: &str = "#404040";
pub(crate) const HUD_TAG_FG: &str = "#a0c4ff";

pub(crate) const KEY_RADIUS: f32 = 8.0;
pub(crate) const KEY_PAD_X: f32 = 6.0;
pub(crate) const KEY_PAD_Y: f32 = 2.0;
pub(crate) const HUD_RADIUS: f32 = 14.0;

// Serde default functions
pub(crate) const fn default_font_size() -> f32 {
    HUD_FONT_SIZE
}
pub(crate) const fn default_opacity() -> f32 {
    HUD_OPACITY
}
pub(crate) const fn default_key_radius() -> f32 {
    KEY_RADIUS
}
pub(crate) const fn default_key_pad_x() -> f32 {
    KEY_PAD_X
}
pub(crate) const fn default_key_pad_y() -> f32 {
    KEY_PAD_Y
}
pub(crate) const fn default_radius() -> f32 {
    HUD_RADIUS
}

// Notify defaults
pub(crate) const NOTIFY_WIDTH: f32 = 420.0;
pub(crate) const NOTIFY_OPACITY: f32 = 1.0;
pub(crate) const NOTIFY_TIMEOUT: f32 = 1.0;
pub(crate) const NOTIFY_BUFFER: usize = 200;
pub(crate) const NOTIFY_POS: NotifyPos = NotifyPos::Right;

pub(crate) const NOTIFY_RADIUS: f32 = 12.0;

// Serde default functions
pub(crate) const fn default_notify_width() -> f32 {
    NOTIFY_WIDTH
}
pub(crate) const fn default_notify_opacity() -> f32 {
    NOTIFY_OPACITY
}
pub(crate) const fn default_notify_timeout() -> f32 {
    NOTIFY_TIMEOUT
}
pub(crate) const fn default_notify_buffer() -> usize {
    NOTIFY_BUFFER
}
pub(crate) const fn default_notify_radius() -> f32 {
    NOTIFY_RADIUS
}

pub(crate) fn default_notify_info_style() -> RawNotifyWindowStyle {
    RawNotifyWindowStyle {
        bg: Some("#222222".to_string()),
        title_fg: Some("white".to_string()),
        body_fg: Some("#d0d0d0".to_string()),
        title_font_size: Some(14.0),
        title_font_weight: Some(crate::FontWeight::Regular),
        body_font_size: Some(12.0),
        body_font_weight: Some(crate::FontWeight::Regular),
        icon: Some("ℹ".to_string()),
    }
}

pub(crate) fn default_notify_warn_style() -> RawNotifyWindowStyle {
    RawNotifyWindowStyle {
        bg: Some("#442a00".to_string()),
        title_fg: Some("#ffc100".to_string()),
        body_fg: Some("#ffd666".to_string()),
        title_font_size: Some(14.0),
        title_font_weight: Some(crate::FontWeight::Regular),
        body_font_size: Some(12.0),
        body_font_weight: Some(crate::FontWeight::Regular),
        icon: Some("⚠".to_string()),
    }
}

pub(crate) fn default_notify_error_style() -> RawNotifyWindowStyle {
    RawNotifyWindowStyle {
        bg: Some("#3a0000".to_string()),
        title_fg: Some("#ff5f5f".to_string()),
        body_fg: Some("#ffb3b3".to_string()),
        title_font_size: Some(14.0),
        title_font_weight: Some(crate::FontWeight::Regular),
        body_font_size: Some(12.0),
        body_font_weight: Some(crate::FontWeight::Regular),
        // Nerdfont nf-cod-error
        icon: Some("\u{ea87}".to_string()),
    }
}

pub(crate) fn default_notify_success_style() -> RawNotifyWindowStyle {
    RawNotifyWindowStyle {
        bg: Some("#0a1628".to_string()),
        title_fg: Some("#a0c4ff".to_string()),
        body_fg: Some("#d0e1ff".to_string()),
        title_font_size: Some(14.0),
        title_font_weight: Some(crate::FontWeight::Regular),
        body_font_size: Some(12.0),
        body_font_weight: Some(crate::FontWeight::Regular),
        icon: Some("\u{f05d}".to_string()),
    }
}

use crate::{Offset, Pos};

pub(crate) const HUD_POS: Pos = Pos::Center;
pub(crate) const HUD_OFFSET: Offset = Offset { x: 0.0, y: 0.0 };
