use std::{
    collections::{HashMap, hash_map::Entry},
    sync::Arc,
};

use mac_keycode::Chord;
use parking_lot::Mutex;

/// State retained from one initial key down until its matching key up.
#[derive(Debug, Clone)]
struct KeyState {
    /// Chord resolved at key-down time.
    chord: Chord,
    /// Whether OS repeat events should be acted upon.
    repeat_allowed: bool,
    /// Whether a HUD press event entered the UI channel.
    press_notified: bool,
}

/// State returned when a held key is released.
pub(super) struct ReleasedKeyState {
    /// Chord retained from the initial key down.
    pub(super) chord: Chord,
    /// Whether the HUD requires a matching release event.
    pub(super) press_notified: bool,
}

/// Tracks key-down identity, repeat permission, and HUD notification state.
#[derive(Clone, Default)]
pub(super) struct KeyStateTracker {
    /// Per-identifier state for currently held keys.
    states: Arc<Mutex<HashMap<String, KeyState>>>,
}

impl KeyStateTracker {
    /// Create a new key state tracker.
    pub(super) fn new() -> Self {
        Self::default()
    }

    /// Return true if the key is currently considered down.
    pub(super) fn is_down(&self, identifier: &str) -> bool {
        self.states.lock().contains_key(identifier)
    }

    /// Record an initial key down, returning false for a duplicate down.
    pub(super) fn on_key_down(&self, identifier: &str, chord: &Chord) -> bool {
        match self.states.lock().entry(identifier.to_string()) {
            Entry::Vacant(entry) => {
                entry.insert(KeyState {
                    chord: chord.clone(),
                    repeat_allowed: false,
                    press_notified: false,
                });
                true
            }
            Entry::Occupied(_) => false,
        }
    }

    /// Remove and return the state associated with a key up.
    pub(super) fn on_key_up(&self, identifier: &str) -> Option<ReleasedKeyState> {
        self.states
            .lock()
            .remove(identifier)
            .map(|state| ReleasedKeyState {
                chord: state.chord,
                press_notified: state.press_notified,
            })
    }

    /// Return the chord retained for a currently held identifier.
    pub(super) fn held_chord(&self, identifier: &str) -> Option<Chord> {
        self.states
            .lock()
            .get(identifier)
            .map(|state| state.chord.clone())
    }

    /// Record that the HUD press event was enqueued for this held key.
    pub(super) fn mark_press_notified(&self, identifier: &str) {
        if let Some(state) = self.states.lock().get_mut(identifier) {
            state.press_notified = true;
        }
    }

    /// Set whether OS repeat events should be acted upon for this identifier.
    pub(super) fn set_repeat_allowed(&self, identifier: &str, allowed: bool) {
        if let Some(state) = self.states.lock().get_mut(identifier) {
            state.repeat_allowed = allowed;
        }
    }

    /// Return true if repeats are allowed for this identifier.
    pub(super) fn is_repeat_allowed(&self, identifier: &str) -> bool {
        self.states
            .lock()
            .get(identifier)
            .is_some_and(|state| state.repeat_allowed)
    }
}

#[cfg(test)]
mod tests {
    use mac_keycode::Chord;

    use super::KeyStateTracker;

    fn chord(spec: &str) -> Chord {
        Chord::parse(spec).expect("test chord")
    }

    #[test]
    fn duplicate_down_keeps_original_gesture_state() {
        let tracker = KeyStateTracker::new();
        assert!(tracker.on_key_down("a", &chord("a")));
        tracker.set_repeat_allowed("a", true);
        tracker.mark_press_notified("a");

        assert!(!tracker.on_key_down("a", &chord("b")));
        assert_eq!(tracker.held_chord("a"), Some(chord("a")));
        assert!(tracker.is_repeat_allowed("a"));

        let released = tracker.on_key_up("a").expect("released state");
        assert_eq!(released.chord, chord("a"));
        assert!(released.press_notified);
        assert!(!tracker.is_down("a"));
    }
}
