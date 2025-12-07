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
    Thin,
    ExtraLight,
    Light,
    #[default]
    Regular,
    Medium,
    SemiBold,
    Bold,
    ExtraBold,
    Black,
}

/// Screen anchor position for HUD placement.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum Pos {
    #[default]
    Center,
    N,
    NE,
    E,
    SE,
    S,
    SW,
    W,
    NW,
}

/// Pixel offset relative to an anchor position (x moves right, y moves up).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct Offset {
    pub x: f32,
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
    #[serde(alias = "l")]
    Left,
    #[default]
    #[serde(alias = "r")]
    Right,
}

/// Concrete per-window styling with fully parsed colors and sizes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotifyWindowStyle {
    pub bg: (u8, u8, u8),
    pub title_fg: (u8, u8, u8),
    pub body_fg: (u8, u8, u8),
    pub title_font_size: f32,
    pub title_font_weight: FontWeight,
    pub body_font_size: f32,
    pub body_font_weight: FontWeight,
    pub icon: Option<String>,
}

/// Fully resolved notification theme for all kinds (info/warn/error/success).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotifyTheme {
    pub info: NotifyWindowStyle,
    pub warn: NotifyWindowStyle,
    pub error: NotifyWindowStyle,
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
