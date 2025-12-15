//! Core configuration data types used in the config crate.

use serde::{Deserialize, Serialize};

/// Display mode for the HUD.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum Mode {
    /// Full HUD is visible.
    #[default]
    Hud,
    /// HUD is hidden.
    Hide,
    /// Minimal HUD variant.
    Mini,
}

/// Font weight used throughout UI elements.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum FontWeight {
    /// Thin weight.
    Thin,
    /// Extra-light weight.
    ExtraLight,
    /// Light weight.
    Light,
    /// Regular weight.
    #[default]
    Regular,
    /// Medium weight.
    Medium,
    /// Semi-bold weight.
    SemiBold,
    /// Bold weight.
    Bold,
    /// Extra-bold weight.
    ExtraBold,
    /// Black weight.
    Black,
}

/// Screen anchor position for HUD placement.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum Pos {
    /// Center of the active display.
    #[default]
    Center,
    /// North (top center).
    N,
    /// Northeast (top right).
    NE,
    /// East (right center).
    E,
    /// Southeast (bottom right).
    SE,
    /// South (bottom center).
    S,
    /// Southwest (bottom left).
    SW,
    /// West (left center).
    W,
    /// Northwest (top left).
    NW,
}

/// Pixel offset relative to an anchor position (x moves right, y moves up).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct Offset {
    /// Horizontal offset in pixels.
    pub x: f32,
    /// Vertical offset in pixels.
    pub y: f32,
}

impl Default for Offset {
    fn default() -> Self {
        Self { x: 0.0, y: 0.0 }
    }
}

/// Side of the screen used to stack notifications.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum NotifyPos {
    /// Left side of the active display.
    #[serde(alias = "l")]
    Left,
    /// Right side of the active display.
    #[default]
    #[serde(alias = "r")]
    Right,
}

/// Concrete per-window styling with fully parsed colors and sizes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotifyWindowStyle {
    /// Background fill color.
    pub bg: (u8, u8, u8),
    /// Foreground color for title text.
    pub title_fg: (u8, u8, u8),
    /// Foreground color for body text.
    pub body_fg: (u8, u8, u8),
    /// Title font size.
    pub title_font_size: f32,
    /// Title font weight.
    pub title_font_weight: FontWeight,
    /// Body font size.
    pub body_font_size: f32,
    /// Body font weight.
    pub body_font_weight: FontWeight,
    /// Optional icon/glyph shown next to the title.
    pub icon: Option<String>,
}

/// Fully resolved notification theme for all kinds (info/warn/error/success).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotifyTheme {
    /// Style for Info notifications.
    pub info: NotifyWindowStyle,
    /// Style for Warn notifications.
    pub warn: NotifyWindowStyle,
    /// Style for Error notifications.
    pub error: NotifyWindowStyle,
    /// Style for Success notifications.
    pub success: NotifyWindowStyle,
}

impl NotifyTheme {
    /// Pick the appropriate window style for a given notification kind.
    pub fn style_for(&self, kind: hotki_protocol::NotifyKind) -> &NotifyWindowStyle {
        use hotki_protocol::NotifyKind;
        match kind {
            NotifyKind::Info | NotifyKind::Ignore => &self.info,
            NotifyKind::Warn => &self.warn,
            NotifyKind::Error => &self.error,
            NotifyKind::Success => &self.success,
        }
    }
}
