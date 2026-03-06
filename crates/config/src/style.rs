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
    style.hud = raw::apply_optional_overlay(
        overrides.hud.as_option().cloned(),
        &style.hud,
        |hud, base| hud.into_hud_over(base),
    );
    style.notify = raw::apply_optional_overlay(
        overrides.notify.as_option().cloned(),
        &style.notify,
        |notify, base| notify.into_notify_over(base),
    );
    style.selector = raw::apply_optional_overlay(
        overrides.selector.as_option().cloned(),
        &style.selector,
        |selector, base| selector.into_selector_over(base),
    );
    style
}

/// Apply multiple raw overlays left-to-right.
pub fn overlay_all_raw(mut style: Style, overlays: &[raw::RawStyle]) -> Style {
    for overlay in overlays {
        style = overlay_raw(style, overlay);
    }
    style
}
