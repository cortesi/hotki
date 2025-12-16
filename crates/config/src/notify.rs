//! Notification configuration and theme resolution.

use serde::{Deserialize, Serialize};

use crate::{
    FontWeight, NotifyPos, NotifyTheme, NotifyWindowStyle, defaults, parse_rgb,
    raw::{Maybe, RawNotify, RawNotifyStyle, RawNotifyWindowStyle},
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

    /// Convert this section into a raw style overlay.
    pub(crate) fn to_raw_notify(&self) -> RawNotify {
        RawNotify {
            width: Maybe::Value(self.width),
            pos: Maybe::Value(self.pos),
            opacity: Maybe::Value(self.opacity),
            timeout: Maybe::Value(self.timeout),
            buffer: Maybe::Value(self.buffer),
            radius: Maybe::Value(self.radius),
            info: Maybe::Value(raw_notify_style_from_window(&self.info)),
            warn: Maybe::Value(raw_notify_style_from_window(&self.warn)),
            error: Maybe::Value(raw_notify_style_from_window(&self.error)),
            success: Maybe::Value(raw_notify_style_from_window(&self.success)),
        }
    }
}

/// Convert the stored raw window style into the overlay representation used in raw configs.
fn raw_notify_style_from_window(win: &RawNotifyWindowStyle) -> RawNotifyStyle {
    RawNotifyStyle {
        bg: opt_to_maybe(win.bg.as_ref()),
        title_fg: opt_to_maybe(win.title_fg.as_ref()),
        body_fg: opt_to_maybe(win.body_fg.as_ref()),
        title_font_size: opt_to_maybe(win.title_font_size.as_ref()),
        title_font_weight: opt_to_maybe(win.title_font_weight.as_ref()),
        body_font_size: opt_to_maybe(win.body_font_size.as_ref()),
        body_font_weight: opt_to_maybe(win.body_font_weight.as_ref()),
        icon: opt_to_maybe(win.icon.as_ref()),
    }
}

/// Convert an `Option<&T>` into the `Maybe<T>` wrapper type used throughout raw configs.
fn opt_to_maybe<T: Clone>(opt: Option<&T>) -> Maybe<T> {
    match opt {
        Some(v) => Maybe::Value(v.clone()),
        None => Maybe::Unit(()),
    }
}
