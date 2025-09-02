use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use tracing::trace;

use mac_keycode::Chord;

#[derive(Clone)]
struct ActiveRelay {
    chord: Chord,
}

/// Relay handler that forwards key events to the focused process.
#[derive(Clone)]
pub struct RelayHandler {
    active: Arc<Mutex<HashMap<String, ActiveRelay>>>,
    relay_key: Option<relaykey::RelayKey>,
}

impl Default for RelayHandler {
    fn default() -> Self {
        Self::new()
    }
}

impl RelayHandler {
    /// Create a new relay handler.
    pub fn new() -> Self {
        let fake = std::env::var("HOTKI_TEST_FAKE_RELAY").is_ok()
            || std::env::var("HOTKI_TEST_FAKE_BINDINGS").is_ok()
            || cfg!(test);
        let relay_key = if fake {
            None
        } else {
            Some(relaykey::RelayKey::new())
        };
        Self {
            active: Arc::new(Mutex::new(HashMap::new())),
            relay_key,
        }
    }

    /// Start relaying a chord to a pid (posts an initial KeyDown).
    pub fn start_relay(&self, id: String, chord: Chord, pid: i32, is_repeat: bool) {
        if let Some(ref relay) = self.relay_key {
            relay.key_down(pid, chord.clone(), is_repeat);
        }
        self.active
            .lock()
            .unwrap()
            .insert(id, ActiveRelay { chord });
        trace!(pid, "relay_start");
    }

    /// Repeat relay for an active id (posts a repeat KeyDown).
    pub fn repeat_relay(&self, id: &str, pid: i32) -> bool {
        if let Some(a) = self.active.lock().unwrap().get(id) {
            if let Some(ref relay) = self.relay_key {
                relay.key_down(pid, a.chord.clone(), true);
            }
            true
        } else {
            false
        }
    }

    /// Stop relaying for id (posts KeyUp and clears state).
    pub fn stop_relay(&self, id: &str, pid: i32) -> bool {
        if let Some(a) = self.active.lock().unwrap().remove(id) {
            if let Some(ref relay) = self.relay_key {
                relay.key_up(pid, a.chord.clone());
            }
            trace!(pid, "relay_stop");
            true
        } else {
            false
        }
    }

    /// Stop all relays (posts KeyUp for each active id, best-effort).
    pub fn stop_all(&self, pid: i32) {
        let mut map = self.active.lock().unwrap();
        if let Some(ref relay) = self.relay_key {
            for (_id, a) in map.drain() {
                relay.key_up(pid, a.chord.clone());
            }
        } else {
            map.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mac_keycode::{Chord, Key};

    fn chord(key: Key) -> Chord {
        use std::collections::HashSet;
        Chord {
            key,
            modifiers: HashSet::new(),
        }
    }

    #[test]
    fn start_repeat_stop_flow() {
        // Test with fake relay (None) - just verify state management
        let handler = RelayHandler::new();
        let id = "id1".to_string();
        let ch = chord(Key::A);

        handler.start_relay(id.clone(), ch.clone(), 1234, false);
        assert!(handler.active.lock().unwrap().contains_key(&id));

        assert!(handler.repeat_relay(&id, 1234));
        assert!(handler.active.lock().unwrap().contains_key(&id));

        assert!(handler.stop_relay(&id, 1234));
        assert!(!handler.active.lock().unwrap().contains_key(&id));

        // Verify stop_relay returns false for non-existent id
        assert!(!handler.stop_relay(&id, 1234));
    }
}
