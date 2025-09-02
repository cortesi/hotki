use crate::Key;

// Central mapping between enum variants and spec strings for shorthand/non-name specs.
// Extend this list to cover more punctuation or shorthand.
macro_rules! key_spec_map {
    ($m:ident, $arg:tt) => {
        $m! { $arg,
            Digit0 => "0",
            Digit1 => "1",
            Digit2 => "2",
            Digit3 => "3",
            Digit4 => "4",
            Digit5 => "5",
            Digit6 => "6",
            Digit7 => "7",
            Digit8 => "8",
            Digit9 => "9",
            Space => " ",
            Minus => "-",
            Equal => "=",
            LeftBracket => "[",
            RightBracket => "]",
            Backslash => "\\",
            Semicolon => ";",
            Quote => "'",
            Comma => ",",
            Period => ".",
            Slash => "/",
            Grave => "`",
        }
    };
}

macro_rules! to_spec_match {
    ( $key:expr, $( $k:ident => $s:expr, )* ) => {
        match $key {
            $( Key::$k => $s, )*
            _ => $key.name(),
        }
    }
}

macro_rules! from_spec_match {
    ( $s:expr, $( $k:ident => $v:expr, )* ) => {{
        match $s {
            $( $v => Some(Key::$k), )*
            _ => None,
        }
    }}
}

// Aliases that only apply to parsing specs (not emitted by to_spec).
macro_rules! key_spec_aliases {
    ($m:ident, $arg:expr) => {
        $m! { $arg,
            // control/meta keys
            Command => "cmd",
            Control => "ctrl",
            Option => "opt",
            Option => "alt",
            CapsLock => "caps",
            Function => "fn",

            // enter/return/delete variants
            Return => "enter",
            Return => "ret",
            Backslash => "backslash", // spelled-out name for convenience
            Comma => "comma",
            Period => "period",
            Slash => "slash",
            Minus => "minus",
            Equal => "equal",
            Semicolon => "semicolon",
            Quote => "quote",
            Grave => "grave",
            LeftBracket => "leftbracket",
            RightBracket => "rightbracket",
            ForwardDelete => "del",
            Delete => "backspace",

            Escape => "esc",
            Space => "space",

            // arrows and navigation
            LeftArrow => "left",
            RightArrow => "right",
            UpArrow => "up",
            DownArrow => "down",
            PageUp => "pgup",
            PageDown => "pgdn",
            ContextualMenu => "menu",

            // keypad enter alias
            KeypadEnter => "kpenter",
        }
    };
}

/// Parses a key specification into a `Key`.
///
/// First tries a case-insensitive enum name (via `Key::from_name`). If that
/// fails, falls back to shorthand specs like digits and punctuation centrally
/// defined in `key_spec_map`.
pub fn from_spec(s: &str) -> Option<Key> {
    if let Some(k) = Key::from_name(s) {
        return Some(k);
    }
    // First try direct shorthand symbols (digits and punctuation), exact match
    if let some @ Some(_) = key_spec_map!(from_spec_match, s) {
        return some;
    }
    // Then try aliases (case-insensitive words)
    let lowered = s.to_ascii_lowercase();
    key_spec_aliases!(from_spec_match, lowered.as_str())
}

/// Returns the key specification string for a `Key`.
///
/// Uses centrally defined shorthand first (digits, punctuation), then falls
/// back to the enum variant name.
pub fn to_spec(key: Key) -> String {
    let s = key_spec_map!(to_spec_match, key);
    s.to_ascii_lowercase()
}

impl Key {
    /// Parses a key specification string into a `Key`.
    ///
    /// Spec parsing differs from `from_name` as follows:
    /// - Accepts enum variant names in a case-insensitive manner.
    /// - Accepts symbol shorthands for digits and punctuation, and space.
    ///   This includes: 0–9, `-`, `=`, `[`, `]`, `\\`, `;`, `'`, `,`, `.`, `/`, and `` ` ``.
    /// - Accepts common alias words (case-insensitive), including: esc, enter, ret,
    ///   cmd, ctrl, opt, alt, caps, fn, left, right, up, down, pgup, pgdn, menu, kpenter.
    ///   Returns `None` if no mapping matches.
    pub fn from_spec(s: &str) -> Option<Self> {
        from_spec(s)
    }

    /// Returns the key specification string for this `Key`.
    ///
    /// Spec emission differs from `name()` as follows:
    /// - For digits, punctuation, and space, returns the symbol form (e.g., comma emits ",").
    /// - For all other keys, returns the enum variant name, identical to `name()`.
    pub fn to_spec(self) -> String {
        to_spec(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_roundtrip(k: Key) {
        let spec = to_spec(k);
        assert_eq!(
            from_spec(&spec),
            Some(k),
            "roundtrip failed for {} -> {}",
            k.name(),
            spec
        );
    }

    #[test]
    fn digit_roundtrip_and_alias() {
        assert_roundtrip(Key::Digit1);
        assert_eq!(from_spec("1"), Some(Key::Digit1));
        assert_eq!(from_spec("digit1"), Some(Key::Digit1));
    }

    #[test]
    fn punctuation_roundtrip_and_alias() {
        assert_roundtrip(Key::Comma);
        assert_eq!(to_spec(Key::Comma), ",");
        assert_eq!(from_spec(","), Some(Key::Comma));
        assert_eq!(from_spec("comma"), Some(Key::Comma));

        assert_roundtrip(Key::Backslash);
        assert_eq!(from_spec("\\"), Some(Key::Backslash));
        assert_eq!(from_spec("backslash"), Some(Key::Backslash));
    }

    #[test]
    fn letter_roundtrip_and_alias() {
        assert_roundtrip(Key::A);
        assert_eq!(to_spec(Key::A), "a");
        assert_eq!(from_spec("a"), Some(Key::A));
        assert_eq!(from_spec("A"), Some(Key::A));
    }

    #[test]
    fn named_roundtrip_and_alias() {
        assert_roundtrip(Key::Tab);
        assert_eq!(to_spec(Key::Tab), "tab");
        assert_eq!(from_spec("tab"), Some(Key::Tab));

        assert_roundtrip(Key::Space);
        assert_eq!(to_spec(Key::Space), " ");
        assert_eq!(from_spec(" "), Some(Key::Space));
        assert_eq!(from_spec("space"), Some(Key::Space));

        // Additional shorthands
        assert_eq!(from_spec("enter"), Some(Key::Return));
        assert_eq!(from_spec("ret"), Some(Key::Return));
        assert_eq!(from_spec("esc"), Some(Key::Escape));
        assert_eq!(from_spec("cmd"), Some(Key::Command));
        assert_eq!(from_spec("ctrl"), Some(Key::Control));
        assert_eq!(from_spec("opt"), Some(Key::Option));
        assert_eq!(from_spec("alt"), Some(Key::Option));
        assert_eq!(from_spec("left"), Some(Key::LeftArrow));
        assert_eq!(from_spec("right"), Some(Key::RightArrow));
        assert_eq!(from_spec("up"), Some(Key::UpArrow));
        assert_eq!(from_spec("down"), Some(Key::DownArrow));
        assert_eq!(from_spec("pgdn"), Some(Key::PageDown));
        assert_eq!(from_spec("pgup"), Some(Key::PageUp));
        assert_eq!(from_spec("kpenter"), Some(Key::KeypadEnter));
    }
}
