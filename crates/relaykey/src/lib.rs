//! Relays live KeyDown/KeyUp events to macOS applications.
//!
//! A `RelayKey` posts KeyDown/KeyUp events either through the global HID event
//! stream or directly to one process.
//! It forwards inputs directly, including explicit repeat KeyDowns provided
//! by the caller.
//!
//! Events are posted directly; no wrapping or synthetic repeats. Invoke
//! [`RelayKey::key_down`] and [`RelayKey::key_up`] with the same destination for
//! one complete gesture. Process destinations carry chord flags on the main key
//! but do not post separate modifier transitions.
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

/// Destination for one relayed key gesture.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RelayDestination {
    /// Post through the global HID event stream to the focused application.
    Hid,
    /// Post directly to one process without changing application focus.
    Process(i32),
}

/// Abstraction for posting events to the system (overridable in tests).
pub(crate) trait Poster: Send + Sync {
    /// Post a key down for `key`.
    fn post_down(&self, key: &Chord, is_repeat: bool, destination: RelayDestination) -> Result<()>;
    /// Post a key up for `key`.
    fn post_up(&self, key: &Chord, destination: RelayDestination) -> Result<()>;
    /// Post HID modifier changes for `mods`.
    fn post_modifiers(
        &self,
        _mods: &HashSet<Modifier>,
        _down: bool,
        _destination: RelayDestination,
    ) -> Result<()> {
        Ok(())
    }
}

/// Default system poster that uses CoreGraphics to inject events.
struct MacPoster;

impl MacPoster {
    /// Create the HID event source used for injected events.
    fn event_source(&self) -> Result<CGEventSource> {
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
        Ok(source)
    }

    /// Build a raw keyboard event for a virtual keycode.
    fn build_keycode_event(&self, keycode: u16, down: bool) -> Result<cge::CGEvent> {
        let source = self.event_source()?;
        let event =
            match cge::CGEvent::new_keyboard_event(source, cge::CGKeyCode::from(keycode), down) {
                Ok(event) => event,
                Err(_) => {
                    if !permissions::accessibility_ok() {
                        warn!("accessibility_permission_missing_for_event_create");
                        return Err(Error::PermissionDenied("Accessibility"));
                    }
                    return Err(Error::EventCreate);
                }
            };
        Ok(event)
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

/// Attach Hotki's event marker so its own event tap ignores injected events.
fn tag_event(event: &cge::CGEvent) {
    event.set_integer_value_field(cge::EventField::EVENT_SOURCE_USER_DATA, HOTK_TAG);
}

/// Post one event through the selected CoreGraphics delivery path.
fn post_event(event: &cge::CGEvent, destination: RelayDestination) {
    if needs_hotki_tag(destination) {
        tag_event(event);
    }
    match destination {
        RelayDestination::Hid => event.post(cge::CGEventTapLocation::HID),
        RelayDestination::Process(pid) => event.post_to_pid(pid),
    }
}

/// Whether an event needs Hotki's HID-loop marker.
fn needs_hotki_tag(destination: RelayDestination) -> bool {
    matches!(destination, RelayDestination::Hid)
}

/// Map a modifier set to virtual keycodes.
fn mod_keycodes(mods: &HashSet<Modifier>) -> Vec<u16> {
    let mut v = Vec::new();
    for m in [
        Modifier::Control,
        Modifier::RightControl,
        Modifier::Option,
        Modifier::RightOption,
        Modifier::Shift,
        Modifier::RightShift,
        Modifier::Command,
        Modifier::RightCommand,
    ] {
        if mods.contains(&m) {
            v.push(m.keycode());
        }
    }
    v
}

impl Poster for MacPoster {
    fn post_down(&self, key: &Chord, is_repeat: bool, destination: RelayDestination) -> Result<()> {
        trace!(
            code = ?key.key,
            mods = ?key.modifiers,
            is_repeat,
            "post_down"
        );
        let event = self.build_event(key, true, is_repeat)?;
        post_event(&event, destination);
        info!(
            code = ?key.key,
            mods = ?key.modifiers,
            is_repeat,
            ?destination,
            "relayed_key_down"
        );
        Ok(())
    }

    fn post_up(&self, key: &Chord, destination: RelayDestination) -> Result<()> {
        trace!(code = ?key.key, mods = ?key.modifiers, "post_up");
        let event = self.build_event(key, false, false)?;
        post_event(&event, destination);
        info!(code = ?key.key, mods = ?key.modifiers, ?destination, "relayed_key_up");
        Ok(())
    }

    fn post_modifiers(
        &self,
        mods: &HashSet<Modifier>,
        down: bool,
        destination: RelayDestination,
    ) -> Result<()> {
        let mut codes = mod_keycodes(mods);
        if !down {
            // Release in reverse order
            codes.reverse();
        }
        for code in codes {
            let event = self.build_keycode_event(code, down)?;
            post_event(&event, destination);
        }
        Ok(())
    }
}

/// Relayer that forwards live key-down, repeat, and key-up events to an explicit
/// destination.
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
            poster: Arc::new(MacPoster),
        }
    }

    /// Test helper to inject a custom poster.
    #[cfg(test)]
    pub(crate) fn new_with_poster(poster: Arc<dyn Poster>) -> Self {
        Self { poster }
    }

    // No release state to manage in pass-through mode.

    /// Post a key-down or repeat input to `destination`.
    pub fn key_down(
        &self,
        key: &Chord,
        is_repeat: bool,
        destination: RelayDestination,
    ) -> Result<()> {
        trace!(code = ?key.key, mods = ?key.modifiers, is_repeat, ?destination, "on_key_down");
        if !is_repeat
            && matches!(destination, RelayDestination::Hid)
            && let Err(err) = self
                .poster
                .post_modifiers(&key.modifiers, true, destination)
        {
            warn!(?err, "post_modifiers_failed");
        }
        self.poster.post_down(key, is_repeat, destination)
    }

    /// Post a key-up input to `destination`.
    pub fn key_up(&self, chord: &Chord, destination: RelayDestination) -> Result<()> {
        trace!(code = ?chord.key, mods = ?chord.modifiers, ?destination, "on_key_up");
        let res = self.poster.post_up(chord, destination);
        if matches!(destination, RelayDestination::Hid)
            && let Err(err) = self
                .poster
                .post_modifiers(&chord.modifiers, false, destination)
        {
            warn!(?err, "post_modifiers_failed");
        }
        res
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::HashSet,
        sync::{
            Mutex,
            atomic::{AtomicUsize, Ordering},
        },
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
        fn post_down(
            &self,
            _key: &Chord,
            _is_repeat: bool,
            _destination: RelayDestination,
        ) -> Result<()> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        fn post_up(&self, _key: &Chord, _destination: RelayDestination) -> Result<()> {
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
        rk.key_down(&key(Key::A), false, RelayDestination::Hid)
            .unwrap();
        rk.key_up(&key(Key::A), RelayDestination::Hid).unwrap();
        assert_eq!(poster.downs(), 1);
        assert_eq!(poster.ups(), 1);
    }

    #[test]
    fn switch_keys_up_then_down() {
        let poster = Arc::new(CountingPoster::new());
        let rk = RelayKey::new_with_poster(poster.clone());
        rk.key_down(&key(Key::A), false, RelayDestination::Hid)
            .unwrap();
        rk.key_down(&key(Key::B), false, RelayDestination::Hid)
            .unwrap();
        rk.key_up(&key(Key::B), RelayDestination::Hid).unwrap();
        // Pass-through: we post exactly what we're asked to.
        assert_eq!(poster.downs(), 2);
        assert_eq!(poster.ups(), 1);
    }

    #[test]
    fn keyup_without_prior_down_posts_up() {
        let poster = Arc::new(CountingPoster::new());
        let rk = RelayKey::new_with_poster(poster.clone());
        rk.key_up(&key(Key::A), RelayDestination::Hid).unwrap();
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
        fn post_down(
            &self,
            _key: &Chord,
            is_repeat: bool,
            _destination: RelayDestination,
        ) -> Result<()> {
            self.downs.fetch_add(1, Ordering::SeqCst);
            if is_repeat {
                self.repeat_downs.fetch_add(1, Ordering::SeqCst);
            }
            Ok(())
        }
        fn post_up(&self, _key: &Chord, _destination: RelayDestination) -> Result<()> {
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
        let destination = RelayDestination::Process(42);
        rk.key_down(&k, false, destination).unwrap();
        rk.key_down(&k, true, destination).unwrap();
        rk.key_up(&k, destination).unwrap();
        assert_eq!(poster.downs(), 2);
        assert_eq!(poster.repeat_downs(), 1);
        assert_eq!(poster.ups(), 1);
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Posted {
        Modifiers(bool, RelayDestination),
        Down(bool, RelayDestination),
        Up(RelayDestination),
    }

    #[derive(Default)]
    struct RecordingPoster(Mutex<Vec<Posted>>);

    impl RecordingPoster {
        fn events(&self) -> Vec<Posted> {
            self.0.lock().expect("recording poster lock").clone()
        }
    }

    impl Poster for RecordingPoster {
        fn post_down(
            &self,
            _key: &Chord,
            is_repeat: bool,
            destination: RelayDestination,
        ) -> Result<()> {
            self.0
                .lock()
                .expect("recording poster lock")
                .push(Posted::Down(is_repeat, destination));
            Ok(())
        }

        fn post_up(&self, _key: &Chord, destination: RelayDestination) -> Result<()> {
            self.0
                .lock()
                .expect("recording poster lock")
                .push(Posted::Up(destination));
            Ok(())
        }

        fn post_modifiers(
            &self,
            _mods: &HashSet<Modifier>,
            down: bool,
            destination: RelayDestination,
        ) -> Result<()> {
            self.0
                .lock()
                .expect("recording poster lock")
                .push(Posted::Modifiers(down, destination));
            Ok(())
        }
    }

    #[test]
    fn hid_destination_is_preserved_through_balanced_modified_gesture() {
        let poster = Arc::new(RecordingPoster::default());
        let relay = RelayKey::new_with_poster(poster.clone());
        let destination = RelayDestination::Hid;
        let chord = Chord {
            key: Key::A,
            modifiers: HashSet::from([Modifier::Command, Modifier::Shift]),
        };

        relay.key_down(&chord, false, destination).unwrap();
        relay.key_down(&chord, true, destination).unwrap();
        relay.key_up(&chord, destination).unwrap();

        assert_eq!(
            poster.events(),
            vec![
                Posted::Modifiers(true, destination),
                Posted::Down(false, destination),
                Posted::Down(true, destination),
                Posted::Up(destination),
                Posted::Modifiers(false, destination),
            ]
        );
    }

    #[test]
    fn process_destination_posts_only_main_key_events() {
        let poster = Arc::new(RecordingPoster::default());
        let relay = RelayKey::new_with_poster(poster.clone());
        let destination = RelayDestination::Process(731);
        let chord = Chord {
            key: Key::A,
            modifiers: HashSet::from([Modifier::Shift]),
        };

        relay.key_down(&chord, false, destination).unwrap();
        relay.key_down(&chord, true, destination).unwrap();
        relay.key_up(&chord, destination).unwrap();

        assert_eq!(
            poster.events(),
            vec![
                Posted::Down(false, destination),
                Posted::Down(true, destination),
                Posted::Up(destination),
            ]
        );
    }

    #[test]
    fn only_hid_events_need_the_hotki_marker() {
        assert!(needs_hotki_tag(RelayDestination::Hid));
        assert!(!needs_hotki_tag(RelayDestination::Process(731)));
    }

    struct FailingPoster;

    impl Poster for FailingPoster {
        fn post_down(
            &self,
            _key: &Chord,
            _is_repeat: bool,
            _destination: RelayDestination,
        ) -> Result<()> {
            Err(Error::EventCreate)
        }

        fn post_up(&self, _key: &Chord, _destination: RelayDestination) -> Result<()> {
            Ok(())
        }
    }

    #[test]
    fn posting_errors_are_returned_without_losing_release_path() {
        let rk = RelayKey::new_with_poster(Arc::new(FailingPoster));
        let chord = key(Key::A);

        let destination = RelayDestination::Process(7);
        assert_eq!(
            rk.key_down(&chord, false, destination),
            Err(Error::EventCreate)
        );
        assert_eq!(rk.key_up(&chord, destination), Ok(()));
    }

    #[test]
    fn test_mod_keycodes_preserves_both_variants() {
        let mods = HashSet::from([
            Modifier::Shift,
            Modifier::RightShift,
            Modifier::Command,
            Modifier::RightCommand,
        ]);

        let keycodes = mod_keycodes(&mods);
        assert_eq!(keycodes.len(), 4);
        assert!(keycodes.contains(&Modifier::Shift.keycode()));
        assert!(keycodes.contains(&Modifier::RightShift.keycode()));
        assert!(keycodes.contains(&Modifier::Command.keycode()));
        assert!(keycodes.contains(&Modifier::RightCommand.keycode()));
    }
}
