use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::{Duration, Instant},
};

use mac_keycode::Chord;
use tracing::{debug, trace, warn};

use crate::Result;
use crate::deps::{CaptureToken, HotkeyApi};

/// Threshold for warning about slow binding updates that may cause key drops
const BIND_UPDATE_WARN_MS: u64 = 10;

/// Manages hotkey bindings and their lifecycle
pub struct KeyBindingManager {
    api: Arc<dyn HotkeyApi>,
    // Registration maps
    id_map: HashMap<u32, String>,
    /// Map registration id → Chord. Stored alongside id → identifier so the
    /// dispatcher can pass chords directly to the callback without doing a
    /// string→parse roundtrip per event.
    chord_map: HashMap<u32, Chord>,
    /// Map identifier (e.g., "cmd+k") → registration id. Maintained during
    /// updates to avoid rebuilding an inverse map when unregistering, which
    /// reduces allocations and speeds up incremental rebinding.
    inv_map: HashMap<String, u32>,
    last_bound: HashSet<String>,
    /// Guard that keeps capture-all active while present
    capture_guard: Option<Box<dyn CaptureToken>>,
    /// Test mode: when true, simulate registrations without OS intercepts.
    fake: bool,
    next_id: u32,
}

impl KeyBindingManager {
    pub fn new_with_api(api: Arc<dyn HotkeyApi>) -> Self {
        Self {
            api,
            id_map: HashMap::new(),
            chord_map: HashMap::new(),
            inv_map: HashMap::new(),
            last_bound: HashSet::new(),
            capture_guard: None,
            fake: std::env::var("HOTKI_TEST_FAKE_BINDINGS").is_ok() || cfg!(test),
            next_id: 1000,
        }
    }

    /// Update bindings based on the desired key pairs
    /// Returns true if bindings changed
    pub fn update_bindings(&mut self, key_pairs: Vec<(String, Chord)>) -> Result<bool> {
        let start = Instant::now();
        let desired: HashSet<String> = key_pairs.iter().map(|(s, _)| s.clone()).collect();

        if self.last_bound == desired {
            trace!("Bindings unchanged, skipping update");
            return Ok(false);
        }

        // Log what keys are being added and removed (without cloning entire sets)
        let added_keys: Vec<String> = desired.difference(&self.last_bound).cloned().collect();
        let removed_keys: Vec<String> = self.last_bound.difference(&desired).cloned().collect();

        if !added_keys.is_empty() {
            debug!("Adding keys: {:?}", added_keys);
        }
        if !removed_keys.is_empty() {
            debug!("Removing keys: {:?}", removed_keys);
        }

        debug!(
            "Starting binding update: {} -> {} keys",
            self.last_bound.len(),
            key_pairs.len()
        );

        // Log all keys that will be active after this update (debug-level)
        let all_keys: Vec<String> = key_pairs.iter().map(|(k, _)| k.clone()).collect();
        debug!("Keys rebound ({}): {:?}", all_keys.len(), all_keys);

        // Incremental update to avoid any interception gap:
        // 1) Unregister removed keys
        // 2) Register added keys
        // 3) Keep existing keys as-is

        // Step 1: Unregister removed keys
        for ident in removed_keys.iter() {
            if let Some(id) = self.inv_map.remove(ident) {
                if !self.fake {
                    self.api.unregister(id)?;
                }
                self.id_map.remove(&id);
                self.chord_map.remove(&id);
                trace!("Unregistered key: {} (id {})", ident, id);
            }
        }

        // Step 2: Register added keys
        for (ident, chord) in &key_pairs {
            if !self.last_bound.contains(ident) {
                let id = if self.fake {
                    self.next_id += 1;
                    self.next_id
                } else {
                    self.api.intercept(chord.clone())
                };
                self.id_map.insert(id, ident.clone());
                self.chord_map.insert(id, chord.clone());
                self.inv_map.insert(ident.clone(), id);
                trace!("Registered key: {} with id {}", ident, id);
            }
        }

        let elapsed = start.elapsed();
        debug!(
            "Binding update completed in {:?}: {} keys active",
            elapsed,
            key_pairs.len()
        );

        if elapsed > Duration::from_millis(BIND_UPDATE_WARN_MS) {
            warn!("Binding update took {:?}, may cause key drops", elapsed);
        }

        self.last_bound = desired;
        Ok(true)
    }

    /// Enable/disable capture-all mode atomically using a guard.
    pub fn set_capture_all(&mut self, active: bool) {
        if self.fake {
            tracing::debug!("capture_all(fake) {}", active);
            return;
        }
        match (active, self.capture_guard.is_some()) {
            (true, false) => {
                self.capture_guard = Some(self.api.capture_all());
                tracing::debug!("capture_all_enabled");
            }
            (false, true) => {
                self.capture_guard = None; // drop guard
                tracing::debug!("capture_all_disabled");
            }
            _ => {}
        }
    }

    /// Resolve a registration id to (identifier, chord) if it exists.
    pub fn resolve(&self, id: u32) -> Option<(String, Chord)> {
        let ident = self.id_map.get(&id).cloned()?;
        let chord = self.chord_map.get(&id).cloned()?;
        Some((ident, chord))
    }
}

impl KeyBindingManager {
    /// Snapshot current bindings as sorted (identifier, chord) pairs.
    pub fn bindings_snapshot(&self) -> Vec<(String, Chord)> {
        // Use inv_map as the authoritative set of active identifiers → ids
        let mut pairs: Vec<(String, Chord)> = Vec::with_capacity(self.inv_map.len());
        for (ident, id) in self.inv_map.iter() {
            if let Some(ch) = self.chord_map.get(id) {
                pairs.push((ident.clone(), ch.clone()));
            }
        }
        pairs.sort_by(|a, b| a.0.cmp(&b.0));
        pairs
    }
}

impl KeyBindingManager {
    pub(crate) fn id_for_ident(&self, ident: &str) -> Option<u32> {
        self.inv_map.get(ident).copied()
    }
}
