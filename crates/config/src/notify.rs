//! Notification configuration and theme resolution.

use serde::{Deserialize, Serialize};

use crate::{
    FontWeight, NotifyPos, NotifyTheme, NotifyWindowStyle, defaults, parse_rgb,
    raw::RawNotifyWindowStyle,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// Notification configuration section.
pub struct Notify {
    /// Fixed width in pixels for each notification window.
    #[serde(default = "defaults::default_notify_width")]
    pub width: f32,

    /// Screen side where the notification stack is anchored (left or right).
    #[serde(default)]
    pub pos: NotifyPos,

    /// Overall window opacity in the range [0.0, 1.0].
    #[serde(default = "defaults::default_notify_opacity")]
    pub opacity: f32,

    /// Auto-dismiss timeout for a notification, in seconds.
    #[serde(default = "defaults::default_notify_timeout")]
    pub timeout: f32,

    /// Maximum number of notifications kept in the on-screen stack.
    #[serde(default = "defaults::default_notify_buffer")]
    pub buffer: usize,

    /// Corner radius for notification windows.
    #[serde(default = "defaults::default_notify_radius")]
    pub radius: f32,

    /// Styling for Info notifications.
    #[serde(default = "defaults::default_notify_info_style")]
    pub(crate) info: RawNotifyWindowStyle,
    /// Styling for Warn notifications.
    #[serde(default = "defaults::default_notify_warn_style")]
    pub(crate) warn: RawNotifyWindowStyle,
    /// Styling for Error notifications.
    #[serde(default = "defaults::default_notify_error_style")]
    pub(crate) error: RawNotifyWindowStyle,
    /// Styling for Success notifications.
    #[serde(default = "defaults::default_notify_success_style")]
    pub(crate) success: RawNotifyWindowStyle,
}

impl Default for Notify {
    fn default() -> Self {
        Self {
            width: defaults::NOTIFY_WIDTH,
            pos: defaults::NOTIFY_POS,
            opacity: defaults::NOTIFY_OPACITY,
            timeout: defaults::NOTIFY_TIMEOUT,
            buffer: defaults::NOTIFY_BUFFER,
            radius: defaults::NOTIFY_RADIUS,
            info: defaults::default_notify_info_style(),
            warn: defaults::default_notify_warn_style(),
            error: defaults::default_notify_error_style(),
            success: defaults::default_notify_success_style(),
        }
    }
}

/// Resolve a raw notification window style to a concrete style with parsed colors.
fn resolve_notify_style(
    raw: &RawNotifyWindowStyle,
    defaults: &RawNotifyWindowStyle,
) -> NotifyWindowStyle {
    /// Resolve a color from raw/default options with a fallback.
    fn color(raw: &Option<String>, def: &Option<String>) -> (u8, u8, u8) {
        let val = raw.as_deref().or(def.as_deref()).unwrap();
        let fallback = def.as_deref().unwrap();
        parse_rgb(val).unwrap_or_else(|| parse_rgb(fallback).unwrap())
    }
    /// Choose a string value from raw or default.
    fn str_val<'a>(raw: &'a Option<String>, def: &'a Option<String>) -> &'a str {
        raw.as_deref().or(def.as_deref()).unwrap()
    }

    NotifyWindowStyle {
        bg: color(&raw.bg, &defaults.bg),
        title_fg: color(&raw.title_fg, &defaults.title_fg),
        body_fg: color(&raw.body_fg, &defaults.body_fg),
        title_font_size: raw
            .title_font_size
            .or(defaults.title_font_size)
            .unwrap_or(14.0),
        title_font_weight: raw.title_font_weight.unwrap_or(FontWeight::Regular),
        body_font_size: raw
            .body_font_size
            .or(defaults.body_font_size)
            .unwrap_or(12.0),
        body_font_weight: raw.body_font_weight.unwrap_or(FontWeight::Regular),
        icon: Some(str_val(&raw.icon, &defaults.icon).to_string()),
    }
}

impl Notify {
    /// Resolve the effective notification theme by applying defaults and parsing colors.
    pub fn theme(&self) -> NotifyTheme {
        let d = Self::default();
        NotifyTheme {
            info: resolve_notify_style(&self.info, &d.info),
            warn: resolve_notify_style(&self.warn, &d.warn),
            error: resolve_notify_style(&self.error, &d.error),
            success: resolve_notify_style(&self.success, &d.success),
        }
    }
}
