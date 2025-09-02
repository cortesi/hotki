use std::{collections::HashSet, convert::TryFrom};

use crate::Key;

/// Modifier keys available on macOS keyboards.
#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
pub enum Modifier {
    Command,
    Shift,
    Option,
    Control,
    CapsLock,
    Function,
    RightCommand,
    RightShift,
    RightOption,
    RightControl,
}

impl From<Modifier> for Key {
    fn from(m: Modifier) -> Self {
        match m {
            Modifier::Command => Key::Command,
            Modifier::Shift => Key::Shift,
            Modifier::Option => Key::Option,
            Modifier::Control => Key::Control,
            Modifier::CapsLock => Key::CapsLock,
            Modifier::Function => Key::Function,
            Modifier::RightCommand => Key::RightCommand,
            Modifier::RightShift => Key::RightShift,
            Modifier::RightOption => Key::RightOption,
            Modifier::RightControl => Key::RightControl,
        }
    }
}

impl TryFrom<Key> for Modifier {
    type Error = ();
    fn try_from(k: Key) -> Result<Self, Self::Error> {
        match k {
            Key::Command => Ok(Modifier::Command),
            Key::Shift => Ok(Modifier::Shift),
            Key::Option => Ok(Modifier::Option),
            Key::Control => Ok(Modifier::Control),
            Key::CapsLock => Ok(Modifier::CapsLock),
            Key::Function => Ok(Modifier::Function),
            Key::RightCommand => Ok(Modifier::RightCommand),
            Key::RightShift => Ok(Modifier::RightShift),
            Key::RightOption => Ok(Modifier::RightOption),
            Key::RightControl => Ok(Modifier::RightControl),
            _ => Err(()),
        }
    }
}

impl Modifier {
    /// Parses a modifier specification string via key specs, then converts.
    ///
    /// Behavior mirrors `Key::from_spec` and accepts case-insensitive variant
    /// names and common alias words (e.g., cmd/ctrl/opt/alt/caps/fn). If the
    /// parsed key is not a modifier, parsing fails.
    pub fn from_spec(s: &str) -> Option<Self> {
        Key::from_spec(s).and_then(|k| Self::try_from(k).ok())
    }

    /// Returns the canonical spec string for this modifier, always lowercased.
    ///
    /// Canonical short forms:
    /// - Command => "cmd"
    /// - Control => "ctrl"
    /// - Option => "opt"
    ///   Others use their lowercased variant name (e.g., "shift", "capslock").
    pub fn to_spec(self) -> String {
        match self {
            Modifier::Command => "cmd".to_string(),
            Modifier::Control => "ctrl".to_string(),
            Modifier::Option => "opt".to_string(),
            _ => Key::from(self).name().to_ascii_lowercase(),
        }
    }
}

/// Construct a modifier set from macOS CGEventFlags bits.
///
/// Only the primary matching bits are considered here:
/// - Shift (1 << 17)
/// - Control (1 << 18)
/// - Option/Alternate (1 << 19)
/// - Command (1 << 20)
///
/// Returns a set containing the corresponding `Modifier` values.
pub fn modifiers_from_cg_flags(flags: u64) -> HashSet<Modifier> {
    let mut set = HashSet::new();
    if flags & (1 << 17) != 0 {
        set.insert(Modifier::Shift);
    }
    if flags & (1 << 18) != 0 {
        set.insert(Modifier::Control);
    }
    if flags & (1 << 19) != 0 {
        set.insert(Modifier::Option);
    }
    if flags & (1 << 20) != 0 {
        set.insert(Modifier::Command);
    }
    set
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_modifiers() {
        let mods = [
            Modifier::Command,
            Modifier::Shift,
            Modifier::Option,
            Modifier::Control,
            Modifier::CapsLock,
            Modifier::Function,
            Modifier::RightCommand,
            Modifier::RightShift,
            Modifier::RightOption,
            Modifier::RightControl,
        ];
        for m in mods {
            let k: Key = m.into();
            let back = Modifier::try_from(k).expect("should map back");
            assert_eq!(m, back);
        }
    }

    #[test]
    fn modifier_specs() {
        // Parse common aliases
        assert_eq!(Modifier::from_spec("cmd"), Some(Modifier::Command));
        assert_eq!(Modifier::from_spec("ctrl"), Some(Modifier::Control));
        assert_eq!(Modifier::from_spec("alt"), Some(Modifier::Option));
        assert_eq!(Modifier::from_spec("opt"), Some(Modifier::Option));
        assert_eq!(Modifier::from_spec("caps"), Some(Modifier::CapsLock));
        assert_eq!(Modifier::from_spec("fn"), Some(Modifier::Function));

        // to_spec is lowercase with canonical short forms
        assert_eq!(Modifier::Command.to_spec(), "cmd");
        assert_eq!(Modifier::Shift.to_spec(), "shift");
    }
}
