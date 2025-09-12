use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use mac_keycode::Chord;
use tracing::trace;

#[derive(Clone)]
struct ActiveRelay {
    chord: Chord,
    pid: i32,
}

/// Relay handler that forwards key events to the focused process.
#[derive(Clone)]
pub struct RelayHandler {
    active: Arc<Mutex<HashMap<String, ActiveRelay>>>,
    relay_key: Option<relaykey::RelayKey>,
}

impl Default for RelayHandler {
    fn default() -> Self {
        Self::new_with_enabled(true)
    }
}

impl RelayHandler {
    /// Create a new relay handler with relay enabled/disabled.
    pub fn new_with_enabled(enabled: bool) -> Self {
        let relay_key = if enabled {
            Some(relaykey::RelayKey::new())
        } else {
            None
        };
        Self {
            active: Arc::new(Mutex::new(HashMap::new())),
            relay_key,
        }
    }
    /// Create a relay handler with relays enabled (production default).
    pub fn new() -> Self {
        Self::new_with_enabled(true)
    }

    /// Start relaying a chord to a pid (posts an initial KeyDown).
    pub fn start_relay(&self, id: String, chord: Chord, pid: i32, is_repeat: bool) {
        if let Some(ref relay) = self.relay_key {
            relay.key_down(pid, chord.clone(), is_repeat);
        }
        self.active
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id.clone(), ActiveRelay { chord, pid });
        trace!(pid, id = %id, "relay_start");
    }

    /// Repeat relay for an active id (posts a repeat KeyDown).
    pub fn repeat_relay(&self, id: &str, pid: i32) -> bool {
        if let Some(a) = self
            .active
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(id)
            .cloned()
        {
            if let Some(ref relay) = self.relay_key {
                relay.key_down(pid, a.chord, true);
            }
            true
        } else {
            false
        }
    }

    /// Stop relaying for id (posts KeyUp and clears state).
    pub fn stop_relay(&self, id: &str, pid: i32) -> bool {
        if let Some(a) = self
            .active
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(id)
        {
            if let Some(ref relay) = self.relay_key {
                // Use the original pid to ensure the key-up matches the key-down target.
                let target_pid = if a.pid != -1 { a.pid } else { pid };
                relay.key_up(target_pid, a.chord);
            }
            trace!(pid = a.pid, id = %id, "relay_stop");
            true
        } else {
            false
        }
    }

    /// Stop all relays (posts KeyUp for each active id, best-effort).
    pub fn stop_all(&self, _pid: i32) {
        let mut map = self.active.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(ref relay) = self.relay_key {
            for (id, a) in map.drain() {
                relay.key_up(a.pid, a.chord);
                trace!(pid = a.pid, id = %id, "relay_stop_all_up");
            }
        } else {
            map.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use mac_keycode::{Chord, Key};

    use super::*;

    fn chord(key: Key) -> Chord {
        use std::collections::HashSet;
        Chord {
            key,
            modifiers: HashSet::new(),
        }
    }

    #[test]
    fn start_repeat_stop_flow() {
        // Test with relay disabled (no OS keystrokes) - just verify state management
        let handler = RelayHandler::new_with_enabled(false);
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
