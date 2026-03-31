//! Relays live KeyDown/KeyUp events to the focused macOS app.
//!
//! A `RelayKey` posts KeyDown/KeyUp events to the focused macOS app.
//! It forwards inputs directly, including explicit repeat KeyDowns provided
//! by the caller.
//!
//! Events are posted directly; no wrapping or synthetic repeats. Invoke
//! `on_key_down(chord, is_repeat)` and `on_key_up(chord)` as needed.
#![warn(missing_docs)]
#![warn(unsafe_op_in_unsafe_fn)]
use std::{collections::HashSet, sync::Arc};

use core_graphics::{
    event as cge,
    event_source::{CGEventSource, CGEventSourceStateID},
};
use mac_hotkey::HOTK_TAG;
use mac_keycode::{Chord, Modifier};
use tracing::{info, trace, warn};
mod error;
pub use error::{Error, Result};

/// Abstraction for posting events to the system (overridable in tests).
pub(crate) trait Poster: Send + Sync {
    /// Post a key down for `key`.
    fn post_down(&self, key: &Chord, is_repeat: bool) -> Result<()>;
    /// Post a key up for `key`.
    fn post_up(&self, key: &Chord) -> Result<()>;
    /// Post modifier changes for `mods`.
    fn post_modifiers(&self, _mods: &HashSet<Modifier>, _down: bool) -> Result<()> {
        Ok(())
    }
}

/// Default system poster that uses CoreGraphics to inject events.
struct MacPoster {
    /// When true, do not set the `HOTK_TAG` on injected events so upstream
    /// taps can observe them (used by tools/smoketests).
    untagged: bool,
}

impl MacPoster {
    /// Build a raw keyboard event for a virtual keycode.
    fn build_keycode_event(&self, keycode: u16, down: bool) -> Result<cge::CGEvent> {
        let source = match CGEventSource::new(CGEventSourceStateID::HIDSystemState) {
            Ok(s) => s,
            Err(_) => {
                if !permissions::accessibility_ok() {
                    warn!("accessibility_permission_missing_for_event_source");
                    return Err(Error::PermissionDenied("Accessibility"));
                }
                return Err(Error::EventSource);
            }
        };
        let e = match cge::CGEvent::new_keyboard_event(source, cge::CGKeyCode::from(keycode), down)
        {
            Ok(e) => e,
            Err(_) => {
                if !permissions::accessibility_ok() {
                    warn!("accessibility_permission_missing_for_event_create");
                    return Err(Error::PermissionDenied("Accessibility"));
                }
                return Err(Error::EventCreate);
            }
        };
        // Tag injected events unless explicitly untagged
        if !self.untagged {
            e.set_integer_value_field(cge::EventField::EVENT_SOURCE_USER_DATA, HOTK_TAG);
        }
        Ok(e)
    }

    /// Build a keyboard event for a `Chord` including modifiers and repeat flag.
    fn build_event(&self, chord: &Chord, down: bool, is_repeat: bool) -> Result<cge::CGEvent> {
        let e = self.build_keycode_event(chord.key as u16, down)?;
        // Apply modifier flags
        let bits: u64 = chord
            .modifiers
            .iter()
            .fold(0_u64, |acc, m| acc | m.cg_flag_bits());
        e.set_flags(cge::CGEventFlags::from_bits_retain(bits));
        if is_repeat {
            e.set_integer_value_field(cge::EventField::KEYBOARD_EVENT_AUTOREPEAT, 1);
        }
        Ok(e)
    }
}

/// Pick the left or right variant of a modifier pair, preferring left unless only right is present.
fn choose_modifier(mods: &HashSet<Modifier>, left: Modifier, right: Modifier) -> Option<Modifier> {
    if mods.contains(&left) {
        Some(left)
    } else if mods.contains(&right) {
        Some(right)
    } else {
        None
    }
}

/// Map a modifier set to virtual keycodes (left-side by default).
fn mod_keycodes(mods: &HashSet<Modifier>) -> Vec<u16> {
    let mut v = Vec::new();
    for (left, right) in [
        (Modifier::Control, Modifier::RightControl),
        (Modifier::Option, Modifier::RightOption),
        (Modifier::Shift, Modifier::RightShift),
        (Modifier::Command, Modifier::RightCommand),
    ] {
        if let Some(chosen) = choose_modifier(mods, left, right) {
            v.push(chosen.keycode());
        }
    }
    v
}

impl Poster for MacPoster {
    fn post_down(&self, key: &Chord, is_repeat: bool) -> Result<()> {
        trace!(
            code = ?key.key,
            mods = ?key.modifiers,
            is_repeat,
            "post_down"
        );
        let e = self.build_event(key, true, is_repeat)?;
        e.post(cge::CGEventTapLocation::HID);
        info!(
            code = ?key.key,
            mods = ?key.modifiers,
            is_repeat,
            "relayed_key_down"
        );
        Ok(())
    }

    fn post_up(&self, key: &Chord) -> Result<()> {
        trace!(code = ?key.key, mods = ?key.modifiers, "post_up");
        let e = self.build_event(key, false, false)?;
        e.post(cge::CGEventTapLocation::HID);
        info!(code = ?key.key, mods = ?key.modifiers, "relayed_key_up");
        Ok(())
    }

    fn post_modifiers(&self, mods: &HashSet<Modifier>, down: bool) -> Result<()> {
        let mut codes = mod_keycodes(mods);
        if !down {
            // Release in reverse order
            codes.reverse();
        }
        for code in codes {
            let e = self.build_keycode_event(code, down)?;
            e.post(cge::CGEventTapLocation::HID);
        }
        Ok(())
    }
}

/// Stateful relayer that forwards live key Down/Up events to the
/// foreground application, ensuring only one relayed key is held at a time.
#[derive(Clone)]
pub struct RelayKey {
    /// Backend responsible for posting events to the OS.
    poster: Arc<dyn Poster>,
}

impl Default for RelayKey {
    fn default() -> Self {
        Self::new()
    }
}

impl RelayKey {
    /// Create a new relayer with no held key and repeats disabled.
    pub fn new() -> Self {
        Self {
            poster: Arc::new(MacPoster { untagged: false }),
        }
    }

    /// Test helper to inject a custom poster.
    #[cfg(test)]
    pub(crate) fn new_with_poster(poster: Arc<dyn Poster>) -> Self {
        Self { poster }
    }

    // No release state to manage in pass-through mode.

    /// Convenience for handling a key-down input.
    pub fn key_down(&self, key: &Chord, is_repeat: bool) -> Result<()> {
        trace!(code = ?key.key, mods = ?key.modifiers, is_repeat, "on_key_down");
        if !is_repeat && let Err(err) = self.poster.post_modifiers(&key.modifiers, true) {
            warn!(?err, "post_modifiers_failed");
        }
        self.poster.post_down(key, is_repeat)
    }

    /// Convenience for handling a key-up input.
    pub fn key_up(&self, chord: &Chord) -> Result<()> {
        trace!(code = ?chord.key, mods = ?chord.modifiers, "on_key_up");
        let res = self.poster.post_up(chord);
        if let Err(err) = self.poster.post_modifiers(&chord.modifiers, false) {
            warn!(?err, "post_modifiers_failed");
        }
        res
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::HashSet,
        sync::atomic::{AtomicUsize, Ordering},
    };

    use mac_keycode::Key;

    use super::*;

    struct CountingPoster(AtomicUsize, AtomicUsize);

    impl CountingPoster {
        fn new() -> Self {
            Self(AtomicUsize::new(0), AtomicUsize::new(0))
        }
        fn downs(&self) -> usize {
            self.0.load(Ordering::SeqCst)
        }
        fn ups(&self) -> usize {
            self.1.load(Ordering::SeqCst)
        }
    }

    impl Poster for CountingPoster {
        fn post_down(&self, _key: &Chord, _is_repeat: bool) -> Result<()> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        fn post_up(&self, _key: &Chord) -> Result<()> {
            self.1.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    fn key(code: Key) -> Chord {
        Chord {
            key: code,
            modifiers: HashSet::new(),
        }
    }

    #[test]
    fn basic_down_up_no_repeat() {
        let poster = Arc::new(CountingPoster::new());
        let rk = RelayKey::new_with_poster(poster.clone());
        rk.key_down(&key(Key::A), false).unwrap();
        rk.key_up(&key(Key::A)).unwrap();
        assert_eq!(poster.downs(), 1);
        assert_eq!(poster.ups(), 1);
    }

    #[test]
    fn switch_keys_up_then_down() {
        let poster = Arc::new(CountingPoster::new());
        let rk = RelayKey::new_with_poster(poster.clone());
        rk.key_down(&key(Key::A), false).unwrap();
        rk.key_down(&key(Key::B), false).unwrap();
        rk.key_up(&key(Key::B)).unwrap();
        // Pass-through: we post exactly what we're asked to.
        assert_eq!(poster.downs(), 2);
        assert_eq!(poster.ups(), 1);
    }

    #[test]
    fn keyup_without_prior_down_posts_up() {
        let poster = Arc::new(CountingPoster::new());
        let rk = RelayKey::new_with_poster(poster.clone());
        rk.key_up(&key(Key::A)).unwrap();
        assert_eq!(poster.downs(), 0);
        assert_eq!(poster.ups(), 1);
    }

    struct TrackPoster {
        downs: AtomicUsize,
        repeat_downs: AtomicUsize,
        ups: AtomicUsize,
    }
    impl TrackPoster {
        fn new() -> Self {
            Self {
                downs: AtomicUsize::new(0),
                repeat_downs: AtomicUsize::new(0),
                ups: AtomicUsize::new(0),
            }
        }
        fn downs(&self) -> usize {
            self.downs.load(Ordering::SeqCst)
        }
        fn repeat_downs(&self) -> usize {
            self.repeat_downs.load(Ordering::SeqCst)
        }
        fn ups(&self) -> usize {
            self.ups.load(Ordering::SeqCst)
        }
    }
    impl Poster for TrackPoster {
        fn post_down(&self, _key: &Chord, is_repeat: bool) -> Result<()> {
            self.downs.fetch_add(1, Ordering::SeqCst);
            if is_repeat {
                self.repeat_downs.fetch_add(1, Ordering::SeqCst);
            }
            Ok(())
        }
        fn post_up(&self, _key: &Chord) -> Result<()> {
            self.ups.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    #[test]
    fn repeats_are_forwarded() {
        let poster = Arc::new(TrackPoster::new());
        let rk = RelayKey::new_with_poster(poster.clone());
        let k = Chord {
            key: Key::RightArrow,
            modifiers: HashSet::new(),
        };
        rk.key_down(&k, false).unwrap();
        rk.key_down(&k, true).unwrap();
        rk.key_up(&k).unwrap();
        assert_eq!(poster.downs(), 2);
        assert_eq!(poster.repeat_downs(), 1);
        assert_eq!(poster.ups(), 1);
    }
}
