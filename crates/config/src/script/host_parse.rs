//! Shared parsing helpers for Luau host values.

use mac_keycode::Chord;
use ruau::vm::{RuntimeError, Scope, ScopedValue, serde::from_scoped_value};
use serde::Deserialize;

use super::Binding;
use crate::NotifyKind;

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
/// Software-repeat options parsed from Luau binding tables.
pub(super) struct RepeatOptionsSpec {
    /// Optional initial repeat delay in milliseconds.
    pub(super) delay_ms: Option<u64>,
    /// Optional repeat interval in milliseconds.
    pub(super) interval_ms: Option<u64>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
/// Shell action modifiers parsed from Luau tables.
pub(super) struct ShellOptionsSpec {
    /// Notification kind used for successful shell exits.
    pub(super) ok_notify: Option<NotifyKind>,
    /// Notification kind used for failing shell exits.
    pub(super) err_notify: Option<NotifyKind>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
/// Common binding options parsed from Luau tables.
pub(super) struct BindingOptionsSpec {
    /// Whether the binding should be hidden from the HUD.
    hidden: Option<bool>,
    /// Whether the binding should be inherited by child modes.
    global: Option<bool>,
    /// Whether the binding suppresses auto-exit after execution.
    stay: Option<bool>,
}

impl BindingOptionsSpec {
    /// Overlay explicit fields on inherited binding defaults.
    pub(super) fn merged_with(&self, explicit: Option<&Self>) -> Self {
        Self {
            hidden: explicit.and_then(|options| options.hidden).or(self.hidden),
            global: explicit.and_then(|options| options.global).or(self.global),
            stay: explicit.and_then(|options| options.stay).or(self.stay),
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
/// Submenu-specific binding options parsed from Luau tables.
pub(super) struct SubmenuOptionsSpec {
    /// Embedded binding options shared with primitive bindings.
    #[serde(flatten)]
    pub(super) binding: BindingOptionsSpec,
    /// Whether entering the submenu enables capture-all behavior.
    pub(super) capture: Option<bool>,
}

/// Deserialize an optional Luau record, treating `nil` as `None`.
pub(super) fn parse_optional<'s, T>(
    scope: &Scope<'s>,
    value: ScopedValue<'s>,
) -> Result<Option<T>, RuntimeError>
where
    T: for<'de> Deserialize<'de>,
{
    if matches!(value, ScopedValue::Nil) {
        return Ok(None);
    }
    from_scoped_value(scope, value)
        .map(Some)
        .map_err(|err| RuntimeError::runtime(err.message()))
}

/// Parse a hotkey chord string into a normalized `Chord`.
pub(super) fn parse_chord(spec: &str) -> Result<Chord, RuntimeError> {
    Chord::parse(spec).ok_or_else(|| RuntimeError::runtime(format!("invalid chord string: {spec}")))
}

/// Apply parsed Luau binding options to a binding.
pub(super) fn apply_binding_options(binding: &mut Binding, options: Option<BindingOptionsSpec>) {
    let Some(options) = options else {
        return;
    };

    binding.flags.hidden = options.hidden.unwrap_or(false);
    binding.flags.global = options.global.unwrap_or(false);
    binding.flags.stay = options.stay.unwrap_or(false);
}
