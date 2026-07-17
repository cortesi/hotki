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

impl TryFrom<Scancode> for Key {
    type Error = ();

    fn try_from(value: Scancode) -> Result<Self, Self::Error> {
        Key::from_keycode(value).ok_or(())
    }
}

impl From<Key> for Scancode {
    fn from(k: Key) -> Self {
        k as u16
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn representative_keys_roundtrip_through_scancodes() {
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
        for key in samples {
            let scancode = Scancode::from(key);
            assert_eq!(Key::try_from(scancode), Ok(key));
            assert_eq!(Scancode::from(Key::try_from(scancode).unwrap()), scancode);
        }
    }

    #[test]
    fn unknown_scancode_is_rejected() {
        assert_eq!(Key::try_from(0xFFFF), Err(()));
    }
}
