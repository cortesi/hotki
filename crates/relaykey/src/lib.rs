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
use libc::pid_t;
use mac_keycode::{Chord, Modifier};
use tracing::{info, trace, warn};

pub(crate) type Result<T> = std::result::Result<T, Error>;

#[derive(thiserror::Error, Debug, Clone, PartialEq, Eq)]
pub(crate) enum Error {
    #[error("Failed to create CGEventSource")]
    EventSource,
    #[error("Failed to create CGEvent")]
    EventCreate,
    #[error("Permission denied: {0}")]
    PermissionDenied(&'static str),
}

pub(crate) trait Poster: Send + Sync {
    fn post_down(&self, pid: pid_t, key: &Chord, is_repeat: bool) -> Result<()>;
    fn post_up(&self, pid: pid_t, key: &Chord) -> Result<()>;
    fn post_modifiers(&self, _pid: pid_t, _mods: &HashSet<Modifier>, _down: bool) -> Result<()> {
        Ok(())
    }
}

struct MacPoster {
    /// When true, do not set the HOTK_TAG on injected events so upstream
    /// taps can observe them (used by tools/smoketests).
    untagged: bool,
}

impl MacPoster {
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
            e.set_integer_value_field(cge::EventField::EVENT_SOURCE_USER_DATA, eventtag::HOTK_TAG);
        }
        Ok(e)
    }
    fn build_event(&self, chord: &Chord, down: bool, is_repeat: bool) -> Result<cge::CGEvent> {
        // Create event source inline - it's lightweight
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
        let e = match cge::CGEvent::new_keyboard_event(
            source,
            cge::CGKeyCode::from(chord.key as u16),
            down,
        ) {
            Ok(e) => e,
            Err(_) => {
                if !permissions::accessibility_ok() {
                    warn!("accessibility_permission_missing_for_event_create");
                    return Err(Error::PermissionDenied("Accessibility"));
                }
                return Err(Error::EventCreate);
            }
        };
        let mut bits: u64 = 0;
        if !chord.modifiers.is_empty() {
            let m = &chord.modifiers;
            if m.contains(&Modifier::Control) || m.contains(&Modifier::RightControl) {
                bits |= 1 << 18;
            }
            if m.contains(&Modifier::Option) || m.contains(&Modifier::RightOption) {
                bits |= 1 << 19;
            }
            if m.contains(&Modifier::Shift) || m.contains(&Modifier::RightShift) {
                bits |= 1 << 17;
            }
            if m.contains(&Modifier::Command) || m.contains(&Modifier::RightCommand) {
                bits |= 1 << 20;
            }
        }
        e.set_flags(cge::CGEventFlags::from_bits_retain(bits));
        // Tag all injected events unless explicitly untagged.
        if !self.untagged {
            e.set_integer_value_field(cge::EventField::EVENT_SOURCE_USER_DATA, eventtag::HOTK_TAG);
        }
        if is_repeat {
            e.set_integer_value_field(cge::EventField::KEYBOARD_EVENT_AUTOREPEAT, 1);
        }
        Ok(e)
    }
}

impl Poster for MacPoster {
    fn post_down(&self, pid: pid_t, key: &Chord, is_repeat: bool) -> Result<()> {
        trace!(
            code = ?key.key,
            mods = ?key.modifiers,
            is_repeat,
            "post_down"
        );
        let e = self.build_event(key, true, is_repeat)?;
        e.post(cge::CGEventTapLocation::HID);
        info!(
            pid,
            code = ?key.key,
            mods = ?key.modifiers,
            is_repeat,
            "relayed_key_down"
        );
        Ok(())
    }

    fn post_up(&self, pid: pid_t, key: &Chord) -> Result<()> {
        trace!(code = ?key.key, mods = ?key.modifiers, "post_up");
        let e = self.build_event(key, false, false)?;
        e.post(cge::CGEventTapLocation::HID);
        info!(pid, code = ?key.key, mods = ?key.modifiers, "relayed_key_up");
        Ok(())
    }

    fn post_modifiers(&self, _pid: pid_t, mods: &HashSet<Modifier>, down: bool) -> Result<()> {
        // Map modifiers to virtual keycodes (left-side by default)
        fn mod_keycodes(m: &HashSet<Modifier>) -> Vec<u16> {
            let mut v = Vec::new();
            // Use left variants when only generic specified
            if m.contains(&Modifier::Control) || m.contains(&Modifier::RightControl) {
                // Prefer left control unless only right is specified
                if m.contains(&Modifier::RightControl) && !m.contains(&Modifier::Control) {
                    v.push(0x3E); // right control
                } else {
                    v.push(0x3B); // left control
                }
            }
            if m.contains(&Modifier::Option) || m.contains(&Modifier::RightOption) {
                if m.contains(&Modifier::RightOption) && !m.contains(&Modifier::Option) {
                    v.push(0x3D); // right option (alt)
                } else {
                    v.push(0x3A); // left option (alt)
                }
            }
            if m.contains(&Modifier::Shift) || m.contains(&Modifier::RightShift) {
                if m.contains(&Modifier::RightShift) && !m.contains(&Modifier::Shift) {
                    v.push(0x3C); // right shift
                } else {
                    v.push(0x38); // left shift
                }
            }
            if m.contains(&Modifier::Command) || m.contains(&Modifier::RightCommand) {
                if m.contains(&Modifier::RightCommand) && !m.contains(&Modifier::Command) {
                    v.push(0x36); // right command
                } else {
                    v.push(0x37); // left command
                }
            }
            v
        }

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

    /// Create a relayer that does NOT tag events with HOTK_TAG.
    /// Use in tools/smoketests to drive the event tap like real user input.
    pub fn new_unlabeled() -> Self {
        Self {
            poster: Arc::new(MacPoster { untagged: true }),
        }
    }

    /// Test helper to inject a custom poster.
    #[cfg(test)]
    pub(crate) fn new_with_poster(poster: Arc<dyn Poster>) -> Self {
        Self { poster }
    }

    /// Create a RelayKey with a mock poster for testing
    #[cfg(any(test, feature = "test-utils"))]
    pub fn new_with_mock_poster() -> Self {
        Self {
            poster: Arc::new(MockPoster),
        }
    }

    // No release state to manage in pass-through mode.

    /// Convenience for handling a key-down input.
    pub fn key_down(&self, pid: i32, key: Chord, is_repeat: bool) {
        trace!(code = ?key.key, mods = ?key.modifiers, is_repeat, "on_key_down");
        let pid = pid as pid_t;
        if !is_repeat {
            let _ = self.poster.post_modifiers(pid, &key.modifiers, true);
        }
        let _ = self.poster.post_down(pid, &key, is_repeat);
    }

    /// Convenience for handling a key-up input.
    pub fn key_up(&self, pid: i32, chord: Chord) {
        trace!(code = ?chord.key, mods = ?chord.modifiers, "on_key_up");
        let pid = pid as pid_t;
        let _ = self.poster.post_up(pid, &chord);
        let _ = self.poster.post_modifiers(pid, &chord.modifiers, false);
    }
}

#[cfg(any(test, feature = "test-utils"))]
struct MockPoster;

#[cfg(any(test, feature = "test-utils"))]
impl Poster for MockPoster {
    fn post_down(&self, _pid: pid_t, _key: &Chord, _is_repeat: bool) -> Result<()> {
        Ok(())
    }
    fn post_up(&self, _pid: pid_t, _key: &Chord) -> Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use mac_keycode::Key;
    use std::sync::atomic::{AtomicUsize, Ordering};

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
        fn post_down(&self, _pid: pid_t, _key: &Chord, _is_repeat: bool) -> Result<()> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        fn post_up(&self, _pid: pid_t, _key: &Chord) -> Result<()> {
            self.1.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    fn key(code: Key) -> Chord {
        use std::collections::HashSet;
        Chord {
            key: code,
            modifiers: HashSet::new(),
        }
    }

    #[test]
    fn basic_down_up_no_repeat() {
        let poster = Arc::new(CountingPoster::new());
        let rk = RelayKey::new_with_poster(poster.clone());
        rk.key_down(1234, key(Key::A), false);
        /* removed misplaced inner doc block */
        rk.key_up(1234, key(Key::A));
        assert_eq!(poster.downs(), 1);
        assert_eq!(poster.ups(), 1);
    }

    #[test]
    fn switch_keys_up_then_down() {
        let poster = Arc::new(CountingPoster::new());
        let rk = RelayKey::new_with_poster(poster.clone());
        rk.key_down(1234, key(Key::A), false);
        rk.key_down(1234, key(Key::B), false);
        rk.key_up(1234, key(Key::B));
        // Pass-through: we post exactly what we're asked to.
        assert_eq!(poster.downs(), 2);
        assert_eq!(poster.ups(), 1);
    }

    #[test]
    fn keyup_without_prior_down_posts_up() {
        let poster = Arc::new(CountingPoster::new());
        let rk = RelayKey::new_with_poster(poster.clone());
        rk.key_up(1234, key(Key::A));
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
        fn post_down(&self, _pid: pid_t, _key: &Chord, is_repeat: bool) -> Result<()> {
            self.downs.fetch_add(1, Ordering::SeqCst);
            if is_repeat {
                self.repeat_downs.fetch_add(1, Ordering::SeqCst);
            }
            Ok(())
        }
        fn post_up(&self, _pid: pid_t, _key: &Chord) -> Result<()> {
            self.ups.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    #[test]
    fn repeats_are_forwarded() {
        let poster = Arc::new(TrackPoster::new());
        let rk = RelayKey::new_with_poster(poster.clone());
        use std::collections::HashSet;
        let k = Chord {
            key: Key::RightArrow,
            modifiers: HashSet::new(),
        };
        rk.key_down(1234, k.clone(), false);
        rk.key_down(1234, k.clone(), true);
        rk.key_up(1234, k);
        assert_eq!(poster.downs(), 2);
        assert_eq!(poster.repeat_downs(), 1);
        assert_eq!(poster.ups(), 1);
    }
}
