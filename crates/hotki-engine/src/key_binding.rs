use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::{Duration, Instant},
};

use mac_keycode::Chord;
use tracing::{debug, trace, warn};

use crate::{
    BIND_UPDATE_WARN_MS, Result,
    deps::{CaptureGuard, HotkeyApi},
};

#[derive(Debug, Clone)]
struct BindingRegistration {
    id: u32,
    chord: Chord,
}

/// Manages hotkey bindings and their lifecycle
pub struct KeyBindingManager {
    api: Arc<dyn HotkeyApi>,
    /// Active registrations keyed by identifier (for example, `"cmd+k"`).
    bindings: HashMap<String, BindingRegistration>,
    /// Reverse lookup from registration id to identifier for event dispatch.
    idents_by_id: HashMap<u32, String>,
    /// Guard that keeps capture-all active while present
    capture_guard: Option<CaptureGuard>,
    /// Capture-all requested by the engine (tracked even in fake mode).
    capture_all_active: bool,
    /// Test mode: when true, simulate registrations without OS intercepts.
    fake: bool,
    next_id: u32,
}

impl KeyBindingManager {
    pub fn new_with_api(api: Arc<dyn HotkeyApi>) -> Self {
        Self {
            api,
            bindings: HashMap::new(),
            idents_by_id: HashMap::new(),
            capture_guard: None,
            capture_all_active: false,
            fake: false, // default; updated below
            next_id: 1000,
        }
        .with_fake_mode()
    }

    fn with_fake_mode(mut self) -> Self {
        // Decide fake mode based on API type
        self.fake = self.api.is_fake();
        self
    }

    /// Update bindings based on the desired key pairs
    /// Returns true if bindings changed
    pub fn update_bindings(&mut self, key_pairs: Vec<(String, Chord)>) -> Result<bool> {
        let start = Instant::now();
        let desired: HashMap<String, Chord> = key_pairs.into_iter().collect();
        if self.bindings_match(&desired) {
            trace!("Bindings unchanged, skipping update");
            return Ok(false);
        }

        let current: HashSet<String> = self.bindings.keys().cloned().collect();
        let desired_idents: HashSet<String> = desired.keys().cloned().collect();
        let mut replaced_keys = Vec::new();
        for (ident, chord) in &desired {
            if self
                .bindings
                .get(ident)
                .is_some_and(|binding| binding.chord != *chord)
            {
                replaced_keys.push(ident.clone());
            }
        }
        let added_keys: Vec<String> = desired_idents.difference(&current).cloned().collect();
        let removed_keys: Vec<String> = current.difference(&desired_idents).cloned().collect();

        if !added_keys.is_empty() {
            debug!("Adding keys: {:?}", added_keys);
        }
        if !removed_keys.is_empty() {
            debug!("Removing keys: {:?}", removed_keys);
        }
        if !replaced_keys.is_empty() {
            debug!("Rebinding keys: {:?}", replaced_keys);
        }

        debug!(
            "Starting binding update: {} -> {} keys",
            self.bindings.len(),
            desired.len()
        );

        // Log all keys that will be active after this update (debug-level)
        let mut all_keys: Vec<String> = desired.keys().cloned().collect();
        all_keys.sort();
        debug!("Keys rebound ({}): {:?}", all_keys.len(), all_keys);

        // Incremental update to avoid any interception gap:
        // 1) Unregister removed keys
        // 2) Register added/changed keys
        // 3) Keep existing keys as-is

        for ident in removed_keys.iter().chain(replaced_keys.iter()) {
            self.unregister_binding(ident)?;
        }

        for ident in added_keys.iter().chain(replaced_keys.iter()) {
            if let Some(chord) = desired.get(ident) {
                self.register_binding(ident, chord)?;
            }
        }

        let elapsed = start.elapsed();
        debug!(
            "Binding update completed in {:?}: {} keys active",
            elapsed,
            desired.len()
        );

        if elapsed > Duration::from_millis(BIND_UPDATE_WARN_MS) {
            warn!("Binding update took {:?}, may cause key drops", elapsed);
        }

        Ok(true)
    }

    /// Enable/disable capture-all mode atomically using a guard.
    pub fn set_capture_all(&mut self, active: bool) {
        self.capture_all_active = active;
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
        let ident = self.idents_by_id.get(&id)?.clone();
        let binding = self.bindings.get(&ident)?;
        Some((ident, binding.chord.clone()))
    }

    pub(crate) fn capture_all_active(&self) -> bool {
        self.capture_all_active
    }
}

impl KeyBindingManager {
    /// Snapshot current bindings as sorted (identifier, chord) pairs.
    pub fn bindings_snapshot(&self) -> Vec<(String, Chord)> {
        let mut pairs: Vec<(String, Chord)> = self
            .bindings
            .iter()
            .map(|(ident, binding)| (ident.clone(), binding.chord.clone()))
            .collect();
        pairs.sort_by(|a, b| a.0.cmp(&b.0));
        pairs
    }
}

impl KeyBindingManager {
    pub(crate) fn id_for_ident(&self, ident: &str) -> Option<u32> {
        self.bindings.get(ident).map(|binding| binding.id)
    }

    fn bindings_match(&self, desired: &HashMap<String, Chord>) -> bool {
        self.bindings.len() == desired.len()
            && desired.iter().all(|(ident, chord)| {
                self.bindings
                    .get(ident)
                    .is_some_and(|binding| binding.chord == *chord)
            })
    }

    fn register_binding(&mut self, ident: &str, chord: &Chord) -> Result<()> {
        let id = if self.fake {
            self.next_id += 1;
            self.next_id
        } else {
            self.api.intercept(chord.clone())
        };
        self.bindings.insert(
            ident.to_string(),
            BindingRegistration {
                id,
                chord: chord.clone(),
            },
        );
        self.idents_by_id.insert(id, ident.to_string());
        trace!("Registered key: {} with id {}", ident, id);
        Ok(())
    }

    fn unregister_binding(&mut self, ident: &str) -> Result<()> {
        let Some(binding) = self.bindings.remove(ident) else {
            return Ok(());
        };
        if !self.fake {
            self.api.unregister(binding.id)?;
        }
        self.idents_by_id.remove(&binding.id);
        trace!("Unregistered key: {} (id {})", ident, binding.id);
        Ok(())
    }
}
