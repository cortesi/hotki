//! Resolved style aliases and overlay helpers.

use crate::raw;

/// Visual theme configuration grouping HUD and notification settings.
pub type Style = hotki_protocol::Style;

/// HUD configuration section.
pub type Hud = hotki_protocol::HudStyle;

/// Notification configuration section.
pub type Notify = hotki_protocol::NotifyConfig;

/// Selector configuration section.
pub type Selector = hotki_protocol::SelectorStyle;

/// Overlay raw style overrides onto this base style using current values as defaults.
pub fn overlay_raw(mut style: Style, overrides: &raw::RawStyle) -> Style {
    if let Some(hud) = overrides.hud.as_option() {
        style.hud = hud.clone().into_hud_over(&style.hud);
    }
    if let Some(notify) = overrides.notify.as_option() {
        style.notify = notify.clone().into_notify_over(&style.notify);
    }
    if let Some(selector) = overrides.selector.as_option() {
        style.selector = selector.clone().into_selector_over(&style.selector);
    }
    style
}

/// Apply multiple raw overlays left-to-right.
pub fn overlay_all_raw(mut style: Style, overlays: &[raw::RawStyle]) -> Style {
    for overlay in overlays {
        style = overlay_raw(style, overlay);
    }
    style
}
