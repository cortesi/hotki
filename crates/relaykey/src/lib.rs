//! Relays live KeyDown/KeyUp events to macOS applications.
//!
//! A `RelayKey` owns complete keyed gestures posted either through the global
//! HID event stream or directly to one process.
//!
//! Each gesture pins its chord and destination at [`RelayKey::begin`], forwards
//! explicit repeats through [`RelayKey::repeat`], and balances the gesture through
//! [`RelayKey::end`]. Process destinations carry chord flags on the main key but
//! do not post separate modifier transitions.
#![warn(missing_docs)]
#![warn(unsafe_op_in_unsafe_fn)]
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex},
};

use core_graphics::{
    event as cge,
    event_source::{CGEventSource, CGEventSourceStateID},
};
use mac_hotkey::HOTK_TAG;
use mac_keycode::{Chord, Modifier, Scancode};
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
        let e = self.build_keycode_event(Scancode::from(chord.key), down)?;
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

/// Chord and destination retained for one active keyed gesture.
#[derive(Clone)]
struct ActiveGesture {
    /// Chord captured when the gesture began.
    chord: Chord,
    /// Delivery destination pinned when the gesture began.
    destination: RelayDestination,
}

/// Last-owner gesture state and its optional system-event backend.
struct RelayState {
    /// Active gestures indexed by caller-supplied identity.
    active: Mutex<HashMap<String, ActiveGesture>>,
    /// Event backend, or `None` when relay posting is disabled.
    poster: Option<Arc<dyn Poster>>,
}

impl RelayState {
    /// Release and remove every active gesture.
    fn release_all(&self) {
        let mut active = self.active.lock().expect("relay state lock");
        if let Some(poster) = &self.poster {
            for (id, gesture) in active.drain() {
                post_up(poster.as_ref(), &gesture.chord, gesture.destination);
                trace!(?gesture.destination, id = %id, "relay_release_all");
            }
        } else {
            active.clear();
        }
    }
}

impl Drop for RelayState {
    fn drop(&mut self) {
        self.release_all();
    }
}

/// Relayer that owns keyed gestures and pins each one to its initial destination.
#[derive(Clone)]
pub struct RelayKey {
    /// Last-owner state; dropping a clone does not release active gestures.
    state: Arc<RelayState>,
}

impl Default for RelayKey {
    fn default() -> Self {
        Self::new()
    }
}

impl RelayKey {
    /// Create an enabled relayer with no active gestures.
    pub fn new() -> Self {
        Self::with_poster(Some(Arc::new(MacPoster)))
    }

    /// Create a relayer that tracks gestures without posting system events.
    pub fn disabled() -> Self {
        Self::with_poster(None)
    }

    /// Test helper to inject a custom poster.
    #[cfg(test)]
    pub(crate) fn new_with_poster(poster: Arc<dyn Poster>) -> Self {
        Self::with_poster(Some(poster))
    }

    /// Construct a relayer around an optional posting backend.
    fn with_poster(poster: Option<Arc<dyn Poster>>) -> Self {
        Self {
            state: Arc::new(RelayState {
                active: Mutex::new(HashMap::new()),
                poster,
            }),
        }
    }

    /// Begin a keyed gesture and pin its chord and destination.
    pub fn begin(&self, id: String, chord: Chord, destination: RelayDestination) {
        let mut active = self.state.active.lock().expect("relay state lock");
        if let Some(previous) = active.remove(&id)
            && let Some(poster) = &self.state.poster
        {
            post_up(poster.as_ref(), &previous.chord, previous.destination);
        }
        if let Some(poster) = &self.state.poster {
            post_down(poster.as_ref(), &chord, false, destination);
        }
        trace!(?destination, id = %id, "relay_begin");
        active.insert(id, ActiveGesture { chord, destination });
    }

    /// Repeat the gesture identified by `id`.
    pub fn repeat(&self, id: &str) -> bool {
        let active = self.state.active.lock().expect("relay state lock");
        if let Some(gesture) = active.get(id) {
            if let Some(poster) = &self.state.poster {
                post_down(poster.as_ref(), &gesture.chord, true, gesture.destination);
            }
            true
        } else {
            false
        }
    }

    /// End the gesture identified by `id`.
    pub fn end(&self, id: &str) -> bool {
        let gesture = self
            .state
            .active
            .lock()
            .expect("relay state lock")
            .remove(id);
        if let Some(gesture) = gesture {
            if let Some(poster) = &self.state.poster {
                post_up(poster.as_ref(), &gesture.chord, gesture.destination);
            }
            trace!(?gesture.destination, id = %id, "relay_end");
            true
        } else {
            false
        }
    }

    /// Release every active gesture using its pinned chord and destination.
    pub fn release_all(&self) {
        self.state.release_all();
    }
}

/// Post one initial or repeated key-down with destination-specific modifiers.
fn post_down(poster: &dyn Poster, chord: &Chord, is_repeat: bool, destination: RelayDestination) {
    trace!(code = ?chord.key, mods = ?chord.modifiers, is_repeat, ?destination, "on_key_down");
    if !is_repeat
        && matches!(destination, RelayDestination::Hid)
        && let Err(error) = poster.post_modifiers(&chord.modifiers, true, destination)
    {
        warn!(?error, "post_modifiers_failed");
    }
    if let Err(error) = poster.post_down(chord, is_repeat, destination) {
        warn!(?error, "relay_down_failed");
    }
}

/// Post one key-up and balance destination-specific modifiers.
fn post_up(poster: &dyn Poster, chord: &Chord, destination: RelayDestination) {
    trace!(code = ?chord.key, mods = ?chord.modifiers, ?destination, "on_key_up");
    if let Err(error) = poster.post_up(chord, destination) {
        warn!(?error, "relay_up_failed");
    }
    if matches!(destination, RelayDestination::Hid)
        && let Err(error) = poster.post_modifiers(&chord.modifiers, false, destination)
    {
        warn!(?error, "post_modifiers_failed");
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
        rk.begin("a".to_string(), key(Key::A), RelayDestination::Hid);
        assert!(rk.end("a"));
        assert_eq!(poster.downs(), 1);
        assert_eq!(poster.ups(), 1);
    }

    #[test]
    fn multiple_gestures_release_independently() {
        let poster = Arc::new(CountingPoster::new());
        let rk = RelayKey::new_with_poster(poster.clone());
        rk.begin("a".to_string(), key(Key::A), RelayDestination::Hid);
        rk.begin("b".to_string(), key(Key::B), RelayDestination::Hid);
        assert!(rk.end("b"));
        assert_eq!(poster.downs(), 2);
        assert_eq!(poster.ups(), 1);
        rk.release_all();
        assert_eq!(poster.ups(), 2);
    }

    #[test]
    fn ending_unknown_gesture_is_a_noop() {
        let poster = Arc::new(CountingPoster::new());
        let rk = RelayKey::new_with_poster(poster.clone());
        assert!(!rk.end("missing"));
        assert_eq!(poster.downs(), 0);
        assert_eq!(poster.ups(), 0);
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
        rk.begin("arrow".to_string(), k, destination);
        assert!(rk.repeat("arrow"));
        assert!(rk.end("arrow"));
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

        relay.begin("modified".to_string(), chord, destination);
        assert!(relay.repeat("modified"));
        assert!(relay.end("modified"));

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

        relay.begin("process".to_string(), chord, destination);
        assert!(relay.repeat("process"));
        assert!(relay.end("process"));

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
    fn replacing_gesture_balances_previous_destination_first() {
        let poster = Arc::new(RecordingPoster::default());
        let relay = RelayKey::new_with_poster(poster.clone());
        let first = RelayDestination::Process(1);
        let second = RelayDestination::Process(2);

        relay.begin("same".to_string(), key(Key::A), first);
        relay.begin("same".to_string(), key(Key::B), second);
        assert!(relay.end("same"));

        assert_eq!(
            poster.events(),
            vec![
                Posted::Down(false, first),
                Posted::Up(first),
                Posted::Down(false, second),
                Posted::Up(second),
            ]
        );
    }

    #[test]
    fn only_hid_events_need_the_hotki_marker() {
        assert!(needs_hotki_tag(RelayDestination::Hid));
        assert!(!needs_hotki_tag(RelayDestination::Process(731)));
    }

    struct FailingPoster(AtomicUsize);

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
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    #[test]
    fn failed_down_remains_releasable() {
        let poster = Arc::new(FailingPoster(AtomicUsize::new(0)));
        let rk = RelayKey::new_with_poster(poster.clone());
        let chord = key(Key::A);

        let destination = RelayDestination::Process(7);
        rk.begin("failed".to_string(), chord, destination);
        assert!(rk.repeat("failed"));
        assert!(rk.end("failed"));
        assert_eq!(poster.0.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn disabled_relayer_tracks_complete_gestures() {
        let relay = RelayKey::disabled();

        relay.begin("disabled".to_string(), key(Key::A), RelayDestination::Hid);

        assert!(relay.repeat("disabled"));
        assert!(relay.end("disabled"));
        assert!(!relay.repeat("disabled"));
    }

    #[test]
    fn dropping_clone_does_not_release_active_gesture() {
        let poster = Arc::new(CountingPoster::new());
        let relay = RelayKey::new_with_poster(poster.clone());
        relay.begin("a".to_string(), key(Key::A), RelayDestination::Hid);

        drop(relay.clone());

        assert_eq!(poster.ups(), 0);
        assert!(relay.end("a"));
        assert_eq!(poster.ups(), 1);
    }

    #[test]
    fn dropping_last_handle_releases_all_gestures() {
        let poster = Arc::new(CountingPoster::new());
        let relay = RelayKey::new_with_poster(poster.clone());
        relay.begin("a".to_string(), key(Key::A), RelayDestination::Hid);
        relay.begin("b".to_string(), key(Key::B), RelayDestination::Hid);

        drop(relay);

        assert_eq!(poster.ups(), 2);
    }

    struct FailingUpPoster(AtomicUsize);

    impl Poster for FailingUpPoster {
        fn post_down(
            &self,
            _key: &Chord,
            _is_repeat: bool,
            _destination: RelayDestination,
        ) -> Result<()> {
            Ok(())
        }

        fn post_up(&self, _key: &Chord, _destination: RelayDestination) -> Result<()> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Err(Error::EventCreate)
        }
    }

    #[test]
    fn release_all_attempts_every_gesture_after_posting_failure() {
        let poster = Arc::new(FailingUpPoster(AtomicUsize::new(0)));
        let relay = RelayKey::new_with_poster(poster.clone());
        relay.begin("a".to_string(), key(Key::A), RelayDestination::Hid);
        relay.begin("b".to_string(), key(Key::B), RelayDestination::Hid);

        relay.release_all();

        assert_eq!(poster.0.load(Ordering::SeqCst), 2);
        assert!(!relay.end("a"));
        assert!(!relay.end("b"));
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
