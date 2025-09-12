use std::sync::{Arc, Mutex};

/// Aggregates all focus-related state used by the engine.
#[derive(Clone)]
pub struct FocusState {
    /// Cached world PID for the focused application/window.
    pub pid: Arc<Mutex<Option<i32>>>,
    /// Cached world focus context (app, title, pid), updated by World events.
    pub ctx: Arc<Mutex<Option<(String, String, i32)>>>,
    /// Last pid explicitly targeted by a Raise action (hint for Place).
    pub last_target_pid: Arc<Mutex<Option<i32>>>,
    /// If true, poll focus snapshot synchronously at dispatch; else trust last snapshot.
    pub sync_on_dispatch: bool,
}

impl FocusState {
    /// Create a new `FocusState` with the specified dispatch policy.
    pub fn new(sync_on_dispatch: bool) -> Self {
        Self {
            pid: Arc::new(Mutex::new(None)),
            ctx: Arc::new(Mutex::new(None)),
            last_target_pid: Arc::new(Mutex::new(None)),
            sync_on_dispatch,
        }
    }
}
