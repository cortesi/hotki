use std::sync::Arc;

use parking_lot::Mutex;

/// Aggregates focus-related state used by the engine.
///
/// Concurrency notes:
/// - The fields use `parking_lot::Mutex` because they are only
///   accessed in short, non-`async` critical sections.
/// - Do not hold these guards across an `.await`. Copy or clone values out
///   and drop the guard before awaiting to avoid blocking the async runtime.
#[derive(Clone)]
pub struct FocusState {
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
            ctx: Arc::new(Mutex::new(None)),
            last_target_pid: Arc::new(Mutex::new(None)),
            sync_on_dispatch,
        }
    }
}
