use serde::{Deserialize, Serialize};

use crate::ui::NotifyKind;

/// Display mode selection for the HUD.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
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
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
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
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
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
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Offset {
    /// Horizontal offset in pixels.
    pub x: f32,
    /// Vertical offset in pixels.
    pub y: f32,
}

/// Side of the screen used to stack notifications.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
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
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct NotifyWindowStyle {
    /// Background fill color.
    pub bg: (u8, u8, u8),
    /// Foreground color for the notification title text.
    pub title_fg: (u8, u8, u8),
    /// Foreground color for the notification body text.
    pub body_fg: (u8, u8, u8),
    /// Title font size.
    pub title_font_size: f32,
    /// Title font weight.
    pub title_font_weight: FontWeight,
    /// Body font size.
    pub body_font_size: f32,
    /// Body font weight.
    pub body_font_weight: FontWeight,
    /// Optional icon/glyph to show next to the title.
    pub icon: Option<String>,
}

/// Fully resolved notification theme for all kinds (info/warn/error/success).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct NotifyTheme {
    /// Styling for Info notifications.
    pub info: NotifyWindowStyle,
    /// Styling for Warn notifications.
    pub warn: NotifyWindowStyle,
    /// Styling for Error notifications.
    pub error: NotifyWindowStyle,
    /// Styling for Success notifications.
    pub success: NotifyWindowStyle,
}

impl NotifyTheme {
    /// Pick the appropriate window style for a given notification kind.
    pub fn style_for(&self, kind: NotifyKind) -> &NotifyWindowStyle {
        match kind {
            NotifyKind::Info | NotifyKind::Ignore => &self.info,
            NotifyKind::Warn => &self.warn,
            NotifyKind::Error => &self.error,
            NotifyKind::Success => &self.success,
        }
    }
}

impl Default for NotifyTheme {
    fn default() -> Self {
        let mk = |bg, title_fg, body_fg, icon: Option<&str>| NotifyWindowStyle {
            bg,
            title_fg,
            body_fg,
            title_font_size: 14.0,
            title_font_weight: FontWeight::Bold,
            body_font_size: 12.0,
            body_font_weight: FontWeight::Regular,
            icon: icon.map(|s| s.to_string()),
        };
        Self {
            info: mk((34, 34, 34), (255, 255, 255), (255, 255, 255), Some("ℹ")),
            warn: mk((68, 42, 0), (255, 193, 0), (255, 193, 0), Some("⚠")),
            error: mk(
                (58, 0, 0),
                (255, 102, 102),
                (255, 102, 102),
                Some("\u{ea87}"),
            ),
            success: mk(
                (12, 45, 12),
                (139, 255, 139),
                (139, 255, 139),
                Some("\u{f05d}"),
            ),
        }
    }
}

/// Fully resolved notification configuration (layout + per-kind styling).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct NotifyConfig {
    /// Fixed width in pixels for each notification window.
    pub width: f32,
    /// Screen side where the notification stack is anchored (left or right).
    pub pos: NotifyPos,
    /// Overall window opacity in the range [0.0, 1.0].
    pub opacity: f32,
    /// Auto-dismiss timeout for a notification, in seconds.
    pub timeout: f32,
    /// Maximum number of notifications kept in the on-screen stack.
    pub buffer: usize,
    /// Corner radius for notification windows.
    pub radius: f32,
    /// Resolved per-kind styling.
    pub theme: NotifyTheme,
}

impl Default for NotifyConfig {
    fn default() -> Self {
        Self {
            width: 420.0,
            pos: NotifyPos::Right,
            opacity: 0.95,
            timeout: 4.0,
            buffer: 200,
            radius: 12.0,
            theme: NotifyTheme::default(),
        }
    }
}

/// HUD style configuration with parsed colors and typography settings.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct HudStyle {
    /// Display mode selection for the HUD.
    pub mode: Mode,
    /// Screen anchor position for the HUD window.
    pub pos: Pos,
    /// Pixel offset added to the anchored position.
    pub offset: Offset,
    /// Base font size for descriptions and general HUD text.
    pub font_size: f32,
    /// Font weight for title/description text.
    pub title_font_weight: FontWeight,
    /// Font size for key tokens inside their rounded boxes.
    pub key_font_size: f32,
    /// Font weight for non-modifier key tokens.
    pub key_font_weight: FontWeight,
    /// Font size for the tag indicator shown for sub-modes.
    pub tag_font_size: f32,
    /// Font weight for the sub-mode tag indicator.
    pub tag_font_weight: FontWeight,
    /// Foreground color for title/description text.
    pub title_fg: (u8, u8, u8),
    /// HUD background fill color.
    pub bg: (u8, u8, u8),
    /// Foreground color for non-modifier key tokens.
    pub key_fg: (u8, u8, u8),
    /// Background color for non-modifier key tokens.
    pub key_bg: (u8, u8, u8),
    /// Foreground color for modifier key tokens.
    pub mod_fg: (u8, u8, u8),
    /// Font weight for modifier key tokens.
    pub mod_font_weight: FontWeight,
    /// Background color for modifier key tokens.
    pub mod_bg: (u8, u8, u8),
    /// Foreground color for the sub-mode tag indicator.
    pub tag_fg: (u8, u8, u8),
    /// Window opacity in the range [0.0, 1.0].
    pub opacity: f32,
    /// Corner radius for key boxes.
    pub key_radius: f32,
    /// Horizontal padding inside key boxes.
    pub key_pad_x: f32,
    /// Vertical padding inside key boxes.
    pub key_pad_y: f32,
    /// Corner radius for the HUD window itself.
    pub radius: f32,
    /// Text tag shown for sub-modes at the end of rows.
    pub tag_submenu: String,
}

impl Default for HudStyle {
    fn default() -> Self {
        Self {
            mode: Mode::Hud,
            pos: Pos::Center,
            offset: Offset { x: 0.0, y: 0.0 },
            font_size: 14.0,
            title_font_weight: FontWeight::Regular,
            key_font_size: 19.0,
            key_font_weight: FontWeight::Bold,
            tag_font_size: 20.0,
            tag_font_weight: FontWeight::Regular,
            title_fg: (208, 208, 208),
            bg: (16, 16, 16),
            key_fg: (208, 208, 208),
            key_bg: (44, 52, 113),
            mod_fg: (255, 255, 255),
            mod_font_weight: FontWeight::Regular,
            mod_bg: (67, 65, 77),
            tag_fg: (55, 79, 138),
            opacity: 1.0,
            key_radius: 4.0,
            key_pad_x: 6.0,
            key_pad_y: 2.0,
            radius: 8.0,
            tag_submenu: "\u{f035f}".to_string(),
        }
    }
}

/// Effective selector style state computed on the server.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SelectorStyle {
    /// Selector background fill color.
    pub bg: (u8, u8, u8),
    /// Text input background fill color.
    pub input_bg: (u8, u8, u8),
    /// Item background fill color.
    pub item_bg: (u8, u8, u8),
    /// Selected item background fill color.
    pub item_selected_bg: (u8, u8, u8),
    /// Foreground color for matched characters in item labels.
    pub match_fg: (u8, u8, u8),
    /// Border color for the selector window.
    pub border: (u8, u8, u8),
    /// Shadow color for the selector window.
    pub shadow: (u8, u8, u8),
}

impl Default for SelectorStyle {
    fn default() -> Self {
        Self {
            bg: (16, 16, 16),
            input_bg: (26, 26, 26),
            item_bg: (16, 16, 16),
            item_selected_bg: (44, 52, 113),
            match_fg: (160, 196, 255),
            border: (48, 48, 48),
            shadow: (0, 0, 0),
        }
    }
}

/// Effective UI style state computed on the server.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(deny_unknown_fields)]
pub struct Style {
    /// HUD style settings.
    pub hud: HudStyle,
    /// Notification style settings.
    pub notify: NotifyConfig,
    /// Selector style settings.
    pub selector: SelectorStyle,
}
