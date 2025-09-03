//! mac-keycode: Virtual keycodes and specs for macOS.
//!
//! - `Key`: Enum of all macOS virtual keycodes (generated at build time).
//! - `Modifier`: Enum of modifier keys with conversions to/from `Key`.
//! - Spec helpers: `Key::from_spec`, `Key::to_spec`, and
//!   `Modifier::from_spec`, `Modifier::to_spec`.
//!
//! The `Key` enum is generated from the macOS SDK HIToolbox header and
//! assigned the exact hardware codes. Variant names are normalized (ANSI_
//! stripped; digits prefixed with `Digit`). Values are hex and the enum is
//! `repr(u16)`.

mod key;
pub use key::Key;

mod spec;

mod modifiers;
pub use modifiers::{Modifier, modifiers_from_cg_flags};

mod chord;
pub use chord::Chord;

mod scancode;
pub use scancode::Scancode;
