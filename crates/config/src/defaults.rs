//! Defaults and constants for UI configuration.
//!
//! These values seed theme styles and provide serde defaults across the
//! configuration module. Items are `pub` within this private module so that
//! sibling modules in the crate can use them without leaking outside the crate.

use crate::{NotifyPos, raw::RawNotifyWindowStyle};

/// Default tag‑submenu glyph (SF Symbols codepoint).
pub const TAG_SUBMENU: &str = "\u{f035f}";
/// Serde helper returning the default tag‑submenu glyph as a `String`.
pub fn default_tag_submenu() -> String {
    TAG_SUBMENU.to_string()
}

// HUD defaults
/// Default HUD font size in points.
pub const HUD_FONT_SIZE: f32 = 16.0;
/// Default HUD window opacity (0.0–1.0).
pub const HUD_OPACITY: f32 = 1.0;
/// Default HUD title foreground color name.
pub const HUD_TITLE_FG: &str = "white";
/// Default HUD background color (hex RGB).
pub const HUD_BG: &str = "#202020";
/// Default HUD key foreground color.
pub const HUD_KEY_FG: &str = "white";
/// Default HUD key background color.
pub const HUD_KEY_BG: &str = "#303030";
/// Default HUD modifier foreground color.
pub const HUD_MOD_FG: &str = "white";
/// Default HUD modifier background color.
pub const HUD_MOD_BG: &str = "#404040";
/// Default HUD tag foreground color.
pub const HUD_TAG_FG: &str = "#a0c4ff";

/// Default key corner radius (px).
pub const KEY_RADIUS: f32 = 8.0;
/// Default horizontal key padding (px).
pub const KEY_PAD_X: f32 = 6.0;
/// Default vertical key padding (px).
pub const KEY_PAD_Y: f32 = 2.0;
/// Default HUD corner radius (px).
pub const HUD_RADIUS: f32 = 14.0;

// Serde default functions
/// Serde default for HUD font size.
pub const fn default_font_size() -> f32 {
    HUD_FONT_SIZE
}
/// Serde default for HUD opacity.
pub const fn default_opacity() -> f32 {
    HUD_OPACITY
}
/// Serde default for key radius.
pub const fn default_key_radius() -> f32 {
    KEY_RADIUS
}
/// Serde default for horizontal key padding.
pub const fn default_key_pad_x() -> f32 {
    KEY_PAD_X
}
/// Serde default for vertical key padding.
pub const fn default_key_pad_y() -> f32 {
    KEY_PAD_Y
}
/// Serde default for HUD radius.
pub const fn default_radius() -> f32 {
    HUD_RADIUS
}

// Notify defaults
/// Default notify width (px).
pub const NOTIFY_WIDTH: f32 = 420.0;
/// Default notify window opacity (0.0–1.0).
pub const NOTIFY_OPACITY: f32 = 1.0;
/// Default notify auto‑dismiss timeout (seconds).
pub const NOTIFY_TIMEOUT: f32 = 1.0;
/// Default ring buffer length for notifications.
pub const NOTIFY_BUFFER: usize = 200;
/// Default on‑screen position for notifications.
pub const NOTIFY_POS: NotifyPos = NotifyPos::Right;

/// Default notify corner radius (px).
pub const NOTIFY_RADIUS: f32 = 12.0;

// Serde default functions
/// Serde default for notify width.
pub const fn default_notify_width() -> f32 {
    NOTIFY_WIDTH
}
/// Serde default for notify opacity.
pub const fn default_notify_opacity() -> f32 {
    NOTIFY_OPACITY
}
/// Serde default for notify timeout.
pub const fn default_notify_timeout() -> f32 {
    NOTIFY_TIMEOUT
}
/// Serde default for notify buffer length.
pub const fn default_notify_buffer() -> usize {
    NOTIFY_BUFFER
}
/// Serde default for notify corner radius.
pub const fn default_notify_radius() -> f32 {
    NOTIFY_RADIUS
}

/// Default style for informational notifications.
pub fn default_notify_info_style() -> RawNotifyWindowStyle {
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

/// Default style for warning notifications.
pub fn default_notify_warn_style() -> RawNotifyWindowStyle {
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

/// Default style for error notifications.
pub fn default_notify_error_style() -> RawNotifyWindowStyle {
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

/// Default style for success notifications.
pub fn default_notify_success_style() -> RawNotifyWindowStyle {
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

/// Default HUD on‑screen position.
pub const HUD_POS: Pos = Pos::Center;
/// Default HUD offset from position in pixels.
pub const HUD_OFFSET: Offset = Offset { x: 0.0, y: 0.0 };
