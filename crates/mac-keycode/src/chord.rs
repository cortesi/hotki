use std::{collections::HashSet, fmt};

use serde::{Deserialize, Serialize};

use crate::{Key, Modifier};

/// A key chord: a set of modifiers plus a single key.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Chord {
    /// Set of modifier keys held down for this chord.
    pub modifiers: HashSet<Modifier>,
    /// The non-modifier key for this chord.
    pub key: Key,
}

impl Chord {
    /// Parses a chord specification of the form "shift+opt+k".
    ///
    /// - Case-insensitive for both modifiers and the key.
    /// - Components are separated by "+"; the last component is always the key spec.
    /// - Modifiers may use aliases handled by `Modifier::from_spec` (e.g., cmd/ctrl/opt/alt/shift).
    /// - The key accepts the full `Key::from_spec` space (digits, punctuation, aliases, or names).
    pub fn parse(s: &str) -> Option<Self> {
        let mut buf: Vec<&str> = s.split('+').collect();
        if buf.is_empty() {
            return None;
        }
        let key_raw = buf.pop().unwrap(); // keep raw to allow literal space
        let key = if key_raw == " " {
            Key::from_spec(" ")
        } else {
            Key::from_spec(key_raw.trim())
        }?;
        let mut modifiers = HashSet::new();
        for m in buf {
            let mt = m.trim();
            if mt.is_empty() {
                return None;
            }
            let mm = Modifier::from_spec(mt)?;
            modifiers.insert(mm);
        }
        Some(Self { modifiers, key })
    }

    fn modifier_order(m: &Modifier) -> usize {
        match m {
            // Canonical order: Command, Option, Control, Shift, Function, CapsLock, Right*
            Modifier::Command => 0,
            Modifier::Option => 1,
            Modifier::Control => 2,
            Modifier::Shift => 3,
            Modifier::Function => 4,
            Modifier::CapsLock => 5,
            Modifier::RightCommand => 6,
            Modifier::RightControl => 7,
            Modifier::RightOption => 8,
            Modifier::RightShift => 9,
        }
    }

    /// Returns the canonical string form of this chord using:
    /// - Canonical modifier order (Command, Control, Option, Shift, Function, CapsLock, Right*...) and
    /// - Canonical spec name for each component (via Modifier::to_spec and Key::to_spec).
    pub fn to_string_canonical(&self) -> String {
        let mut mods: Vec<Modifier> = self.modifiers.iter().copied().collect();
        mods.sort_by_key(Self::modifier_order);
        let mut out: Vec<String> = Vec::new();
        for m in mods {
            out.push(m.to_spec());
        }
        out.push(self.key.to_spec());
        out.join("+")
    }
}

impl fmt::Display for Chord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_string_canonical())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_basic_chord() {
        let c = Chord::parse("shift+opt+k").expect("parse");
        assert!(c.modifiers.contains(&Modifier::Shift));
        assert!(c.modifiers.contains(&Modifier::Option));
        assert_eq!(c.key, Key::K);
        // Canonical order and lowercase specs
        assert_eq!(c.to_string(), "opt+shift+k");
    }

    #[test]
    fn digit_and_punct() {
        let c1 = Chord::parse("cmd+1").expect("parse");
        assert!(c1.modifiers.contains(&Modifier::Command));
        assert_eq!(c1.key, Key::Digit1);
        assert_eq!(c1.to_string(), "cmd+1");

        let c2 = Chord::parse("ctrl+, ").expect("parse");
        assert!(c2.modifiers.contains(&Modifier::Control));
        assert_eq!(c2.key, Key::Comma);
        assert_eq!(c2.to_string(), "ctrl+,");
    }

    #[test]
    fn idempotence_roundtrip() {
        let inputs = ["shift+opt+k", "CTRL+ALT+Space", "Command+Digit1", "fn+pgdn"];
        for s in inputs {
            let c = Chord::parse(s).expect("parse");
            let spec = c.to_string();
            let c2 = Chord::parse(&spec).expect("reparse");
            assert_eq!(c, c2, "idempotent for {} => {}", s, spec);
        }
    }

    #[test]
    fn parse_no_modifiers_letter() {
        let c = Chord::parse("a").expect("parse");
        assert!(c.modifiers.is_empty());
        assert_eq!(c.key, Key::A);
        assert_eq!(c.to_string(), "a");
    }
}
