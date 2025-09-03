//! Scancodes (macOS hardware virtual keycodes) and conversions.
//!
//! A "scancode" in this crate refers to the macOS hardware virtual keycode:
//! - The integer reported by `NSEvent.keyCode` and by CoreGraphics in the
//!   `kCGKeyboardEventKeycode` field.
//! - The set of constants prefixed `kVK_` in the SDK header
//!   `HIToolbox/Events.h`.
//! - A layout-independent, positional identifier for a physical key — it does
//!   not represent a character and it is specific to macOS (i.e., not a USB HID
//!   usage ID, not a Windows scan code, and not Unicode).

use crate::Key;

/// macOS hardware virtual keycode (`kVK_*`, `NSEvent.keyCode`).
pub type Scancode = u16;

/// Returns true if the scancode maps to a known `Key` variant.
///
/// This function is part of the public API and may not be referenced within
/// this crate itself; suppress the dead_code lint accordingly.
#[allow(dead_code)]
pub fn is_valid(sc: Scancode) -> bool {
    Key::from_scancode(sc).is_some()
}

impl TryFrom<Scancode> for Key {
    type Error = ();
    fn try_from(value: Scancode) -> Result<Self, Self::Error> {
        Key::from_scancode(value).ok_or(())
    }
}

impl From<Key> for Scancode {
    fn from(k: Key) -> Self {
        k as u16
    }
}

impl Key {
    /// Looks up a `Key` from a macOS scancode (hardware virtual keycode).
    pub fn from_scancode(sc: Scancode) -> Option<Self> {
        // Reuse the generated mapping which is based on HIToolbox `kVK_*` values.
        Self::from_keycode(sc)
    }

    /// Returns the scancode (`kVK_*`) for this key.
    pub const fn scancode(self) -> Scancode {
        self as u16
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_all_keys() {
        // Sample a few to keep tests light in this crate. Full mapping is generated
        // and validated indirectly; here we spot-check representative keys.
        let samples = [
            Key::A,
            Key::Digit1,
            Key::Space,
            Key::Return,
            Key::LeftArrow,
            Key::UpArrow,
            Key::F1,
            Key::KeypadEnter,
        ];
        for k in samples {
            let sc = k.scancode();
            assert!(is_valid(sc));
            assert_eq!(Key::from_scancode(sc), Some(k));
            assert_eq!(Key::try_from(sc).ok(), Some(k));
            let back: Scancode = Scancode::from(k);
            assert_eq!(back, sc);
        }

        // Unknown example should be invalid; pick a value outside known range.
        assert_eq!(Key::from_scancode(0xFFFF), None);
        assert!(!is_valid(0xFFFF));
    }
}
