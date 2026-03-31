use std::{collections::HashMap, sync::Arc};

use mac_keycode::Chord;
use parking_lot::Mutex;
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
    /// Start relaying a chord to a pid (posts an initial KeyDown).
    pub fn start_relay(&self, id: String, chord: Chord, pid: i32, is_repeat: bool) {
        if let Some(ref relay) = self.relay_key
            && let Err(e) = relay.key_down(&chord, is_repeat)
        {
            tracing::warn!(?e, "relay_down_failed");
        }
        self.active
            .lock()
            .insert(id.clone(), ActiveRelay { chord, pid });
        trace!(pid, id = %id, "relay_start");
    }

    /// Repeat relay for an active id (posts a repeat KeyDown).
    pub fn repeat_relay(&self, id: &str) -> bool {
        if let Some(a) = self.active.lock().get(id).cloned() {
            if let Some(ref relay) = self.relay_key
                && let Err(e) = relay.key_down(&a.chord, true)
            {
                tracing::warn!(?e, "relay_repeat_failed");
            }
            true
        } else {
            false
        }
    }

    /// Stop relaying for id (posts KeyUp and clears state).
    pub fn stop_relay(&self, id: &str, pid: i32) -> bool {
        if let Some(a) = self.active.lock().remove(id) {
            let target_pid = if a.pid != -1 { a.pid } else { pid };
            if let Some(ref relay) = self.relay_key
                && let Err(e) = relay.key_up(&a.chord)
            {
                tracing::warn!(?e, "relay_up_failed");
            }
            trace!(pid = target_pid, id = %id, "relay_stop");
            true
        } else {
            false
        }
    }

    /// Stop all relays (posts KeyUp for each active id, best-effort).
    pub fn stop_all(&self) {
        let mut map = self.active.lock();
        if let Some(ref relay) = self.relay_key {
            for (id, a) in map.drain() {
                if let Err(e) = relay.key_up(&a.chord) {
                    tracing::warn!(?e, "relay_stop_all_up_failed");
                }
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
        assert!(handler.active.lock().contains_key(&id));

        assert!(handler.repeat_relay(&id));
        assert!(handler.active.lock().contains_key(&id));

        assert!(handler.stop_relay(&id, 1234));
        assert!(!handler.active.lock().contains_key(&id));

        // Verify stop_relay returns false for non-existent id
        assert!(!handler.stop_relay(&id, 1234));
    }
}
