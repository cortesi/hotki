//! Immutable input-health snapshots for the macOS event tap.

use std::{
    process,
    sync::Arc,
    time::{Duration, Instant, SystemTime},
};

use parking_lot::Mutex;

/// Whether this manager observes the physical keyboard.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TapMode {
    /// A production CoreGraphics event tap is installed.
    Physical,
    /// Events can only arrive through the explicit injection API.
    InjectionOnly,
}

/// Current lifecycle of the physical event tap.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TapLifecycle {
    /// The event-tap thread is starting.
    Starting,
    /// The event tap is installed and its run loop is active.
    Running,
    /// No physical event tap is running.
    Stopped,
}

/// Last sampled state of macOS Secure Event Input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SecureInputState {
    /// The production sampler has not observed the platform yet.
    Unknown,
    /// Secure Event Input was inactive at the last observation.
    Inactive,
    /// Secure Event Input was active at the last observation.
    Active,
}

/// Best-effort identity of the application owning Secure Event Input.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SecureInputOwner {
    /// Process identifier reported by the current macOS session.
    pub pid: u32,
    /// AppKit localized name resolved while the process was live.
    pub app_name: String,
}

/// Immutable snapshot of the manager's physical-input health.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ManagerStatus {
    /// Whether this manager has a physical event tap.
    pub tap_mode: TapMode,
    /// Current event-tap lifecycle.
    pub tap_lifecycle: TapLifecycle,
    /// Last sampled Secure Event Input state.
    pub secure_input: SecureInputState,
    /// Best-effort owner observed with active Secure Event Input.
    pub secure_input_owner: Option<SecureInputOwner>,
    /// Number of currently registered hotkeys.
    pub registered_hotkeys: usize,
    /// Number of physical key events observed by the tap.
    pub physical_event_count: u64,
    /// Age of the latest physical key event at snapshot time.
    pub physical_event_age_ms: Option<u64>,
    /// Number of callbacks reporting that macOS disabled the tap.
    pub os_disable_count: u64,
    /// Number of successful tap re-enable checks after those callbacks.
    pub os_reenable_count: u64,
    /// Wall-clock time of the last Secure Input observation.
    pub observed_at_ms: Option<u64>,
    /// PID of the server process that owns this manager.
    pub server_pid: u32,
}

#[derive(Clone, Debug)]
pub(crate) struct PlatformSample {
    pub(crate) secure_input: SecureInputState,
    pub(crate) owner: Option<SecureInputOwner>,
}

#[derive(Debug)]
struct MutableStatus {
    tap_lifecycle: TapLifecycle,
    secure_input: SecureInputState,
    secure_input_owner: Option<SecureInputOwner>,
    physical_event_count: u64,
    last_physical_event: Option<Duration>,
    os_disable_count: u64,
    os_reenable_count: u64,
    observed_at_ms: Option<u64>,
}

/// Shared status recorder and serialized platform sampler.
pub(crate) struct StatusStore {
    mode: TapMode,
    started: Instant,
    sample_lock: Mutex<()>,
    state: Mutex<MutableStatus>,
}

impl StatusStore {
    pub(crate) fn new(mode: TapMode) -> Arc<Self> {
        Arc::new(Self {
            mode,
            started: Instant::now(),
            sample_lock: Mutex::new(()),
            state: Mutex::new(MutableStatus {
                tap_lifecycle: match mode {
                    TapMode::Physical => TapLifecycle::Starting,
                    TapMode::InjectionOnly => TapLifecycle::Stopped,
                },
                secure_input: SecureInputState::Unknown,
                secure_input_owner: None,
                physical_event_count: 0,
                last_physical_event: None,
                os_disable_count: 0,
                os_reenable_count: 0,
                observed_at_ms: None,
            }),
        })
    }

    pub(crate) fn set_lifecycle(&self, lifecycle: TapLifecycle) {
        self.state.lock().tap_lifecycle = lifecycle;
    }

    pub(crate) fn record_physical_event(&self) {
        self.record_physical_event_at(self.started.elapsed());
    }

    fn record_physical_event_at(&self, elapsed: Duration) {
        let mut state = self.state.lock();
        state.physical_event_count = state.physical_event_count.saturating_add(1);
        state.last_physical_event = Some(elapsed);
    }

    pub(crate) fn record_disable(&self) {
        let mut state = self.state.lock();
        state.os_disable_count = state.os_disable_count.saturating_add(1);
    }

    pub(crate) fn record_reenable(&self, enabled: bool) {
        if enabled {
            let mut state = self.state.lock();
            state.os_reenable_count = state.os_reenable_count.saturating_add(1);
        }
    }

    pub(crate) fn sample(&self, registered_hotkeys: usize) -> ManagerStatus {
        if self.mode == TapMode::InjectionOnly {
            return self.cached(registered_hotkeys);
        }

        let _sample_guard = self.sample_lock.lock();
        let sample = crate::sys::sample_platform();
        self.sample_at(
            self.started.elapsed(),
            SystemTime::now(),
            registered_hotkeys,
            sample,
        )
    }

    pub(crate) fn cached(&self, registered_hotkeys: usize) -> ManagerStatus {
        self.snapshot_at(self.started.elapsed(), registered_hotkeys)
    }

    fn sample_at(
        &self,
        elapsed: Duration,
        observed_at: SystemTime,
        registered_hotkeys: usize,
        sample: PlatformSample,
    ) -> ManagerStatus {
        let mut state = self.state.lock();
        state.secure_input = sample.secure_input;
        state.secure_input_owner = sample.owner;
        state.observed_at_ms = observed_at
            .duration_since(SystemTime::UNIX_EPOCH)
            .ok()
            .and_then(|duration| u64::try_from(duration.as_millis()).ok());
        Self::snapshot(self.mode, elapsed, registered_hotkeys, &state)
    }

    fn snapshot_at(&self, elapsed: Duration, registered_hotkeys: usize) -> ManagerStatus {
        Self::snapshot(self.mode, elapsed, registered_hotkeys, &self.state.lock())
    }

    fn snapshot(
        mode: TapMode,
        elapsed: Duration,
        registered_hotkeys: usize,
        state: &MutableStatus,
    ) -> ManagerStatus {
        ManagerStatus {
            tap_mode: mode,
            tap_lifecycle: state.tap_lifecycle,
            secure_input: state.secure_input,
            secure_input_owner: state.secure_input_owner.clone(),
            registered_hotkeys,
            physical_event_count: state.physical_event_count,
            physical_event_age_ms: state.last_physical_event.map(|last| {
                u64::try_from(elapsed.saturating_sub(last).as_millis()).unwrap_or(u64::MAX)
            }),
            os_disable_count: state.os_disable_count,
            os_reenable_count: state.os_reenable_count,
            observed_at_ms: state.observed_at_ms,
            server_pid: process::id(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, SystemTime};

    use super::{
        PlatformSample, SecureInputOwner, SecureInputState, StatusStore, TapLifecycle, TapMode,
    };

    #[test]
    fn status_transitions_use_injected_time_and_sample() {
        let status = StatusStore::new(TapMode::Physical);
        status.set_lifecycle(TapLifecycle::Running);
        status.record_physical_event_at(Duration::from_millis(25));
        status.record_disable();
        status.record_reenable(true);

        let snapshot = status.sample_at(
            Duration::from_millis(125),
            SystemTime::UNIX_EPOCH + Duration::from_millis(2_000),
            3,
            PlatformSample {
                secure_input: SecureInputState::Active,
                owner: Some(SecureInputOwner {
                    pid: 42,
                    app_name: "Terminal".to_string(),
                }),
            },
        );

        assert_eq!(snapshot.tap_lifecycle, TapLifecycle::Running);
        assert_eq!(snapshot.physical_event_count, 1);
        assert_eq!(snapshot.physical_event_age_ms, Some(100));
        assert_eq!(snapshot.os_disable_count, 1);
        assert_eq!(snapshot.os_reenable_count, 1);
        assert_eq!(snapshot.observed_at_ms, Some(2_000));
        assert_eq!(snapshot.registered_hotkeys, 3);
        assert_eq!(snapshot.secure_input_owner.unwrap().pid, 42);
    }

    #[test]
    fn injection_only_stays_unknown_without_an_owner() {
        let status = StatusStore::new(TapMode::InjectionOnly);
        let snapshot = status.sample(2);

        assert_eq!(snapshot.tap_mode, TapMode::InjectionOnly);
        assert_eq!(snapshot.tap_lifecycle, TapLifecycle::Stopped);
        assert_eq!(snapshot.secure_input, SecureInputState::Unknown);
        assert_eq!(snapshot.secure_input_owner, None);
        assert_eq!(snapshot.observed_at_ms, None);
    }

    #[test]
    fn unsuccessful_reenable_is_not_counted() {
        let status = StatusStore::new(TapMode::Physical);
        status.record_disable();
        status.record_reenable(false);

        let snapshot = status.cached(0);
        assert_eq!(snapshot.os_disable_count, 1);
        assert_eq!(snapshot.os_reenable_count, 0);
    }
}
