use std::{
    collections::HashSet,
    sync::{Arc, Mutex},
};

/// Tracks the state of keys to handle key up/down and repeat events properly
#[derive(Clone)]
pub struct KeyStateTracker {
    held: Arc<Mutex<HashSet<String>>>,
    repeat_ok: Arc<Mutex<HashSet<String>>>,
}

impl KeyStateTracker {
    /// Create a new key state tracker.
    pub fn new() -> Self {
        Self {
            held: Arc::new(Mutex::new(HashSet::new())),
            repeat_ok: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    /// Return true if the key is currently considered down.
    pub fn is_down(&self, identifier: &str) -> bool {
        let held = self.held.lock().unwrap();
        held.contains(identifier)
    }

    // back-compat helpers removed (not used)
    /// Record a key down; returns true for the first down, false for repeats.
    pub fn on_key_down(&self, identifier: &str) -> bool {
        let mut held = self.held.lock().unwrap();
        held.insert(identifier.to_string())
    }

    /// Record a key up.
    pub fn on_key_up(&self, identifier: &str) {
        let mut held = self.held.lock().unwrap();
        held.remove(identifier);
        let mut rep = self.repeat_ok.lock().unwrap();
        rep.remove(identifier);
    }

    /// Set whether OS repeat events should be acted upon for this identifier.
    pub fn set_repeat_allowed(&self, identifier: &str, allowed: bool) {
        let mut rep = self.repeat_ok.lock().unwrap();
        if allowed {
            rep.insert(identifier.to_string());
        } else {
            rep.remove(identifier);
        }
    }

    /// Return true if repeats are allowed for this identifier.
    pub fn is_repeat_allowed(&self, identifier: &str) -> bool {
        let rep = self.repeat_ok.lock().unwrap();
        rep.contains(identifier)
    }
}

impl Default for KeyStateTracker {
    fn default() -> Self {
        Self::new()
    }
}
