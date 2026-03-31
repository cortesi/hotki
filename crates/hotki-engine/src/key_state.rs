use std::{collections::HashMap, sync::Arc};

use parking_lot::Mutex;

/// Tracks the state of keys to handle key up/down and repeat events properly
#[derive(Clone)]
pub struct KeyStateTracker {
    /// Maps identifier to whether repeats are allowed
    states: Arc<Mutex<HashMap<String, bool>>>,
}

impl KeyStateTracker {
    /// Create a new key state tracker.
    pub fn new() -> Self {
        Self {
            states: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Return true if the key is currently considered down.
    pub fn is_down(&self, identifier: &str) -> bool {
        self.states.lock().contains_key(identifier)
    }

    /// Record a key down; returns true for the first down, false for repeats.
    pub fn on_key_down(&self, identifier: &str) -> bool {
        let mut states = self.states.lock();
        if states.contains_key(identifier) {
            false
        } else {
            states.insert(identifier.to_string(), false);
            true
        }
    }

    /// Record a key up.
    pub fn on_key_up(&self, identifier: &str) {
        self.states.lock().remove(identifier);
    }

    /// Set whether OS repeat events should be acted upon for this identifier.
    pub fn set_repeat_allowed(&self, identifier: &str, allowed: bool) {
        if let Some(repeat_ok) = self.states.lock().get_mut(identifier) {
            *repeat_ok = allowed;
        }
    }

    /// Return true if repeats are allowed for this identifier.
    pub fn is_repeat_allowed(&self, identifier: &str) -> bool {
        self.states.lock().get(identifier).copied().unwrap_or(false)
    }
}

impl Default for KeyStateTracker {
    fn default() -> Self {
        Self::new()
    }
}
