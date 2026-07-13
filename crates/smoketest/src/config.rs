//! Configuration constants and defaults for smoketests.

use std::time::{Duration, Instant};

/// Default wall-clock budget for one smoketest case.
pub const DEFAULT_RUN_BUDGET_MS: u64 = 10_000;

/// One monotonic deadline shared by a case, its waits, and its watchdog.
#[derive(Debug, Clone, Copy)]
pub struct RunBudget {
    /// Instant when budget accounting began.
    started_at: Instant,
    /// Absolute instant when the run must stop.
    deadline: Instant,
    /// Configured wall-clock allowance in milliseconds.
    total_ms: u64,
}

impl RunBudget {
    /// Start a run budget with the supplied total allowance.
    pub fn new(total_ms: u64) -> Self {
        let started_at = Instant::now();
        Self {
            started_at,
            deadline: started_at + Duration::from_millis(total_ms),
            total_ms,
        }
    }

    /// Configured wall-clock allowance in milliseconds.
    pub fn total_ms(self) -> u64 {
        self.total_ms
    }

    /// Elapsed time since budget accounting began.
    pub fn elapsed(self) -> Duration {
        self.started_at.elapsed()
    }

    /// Remaining duration, or `None` when the budget is exhausted.
    pub fn remaining(self) -> Option<Duration> {
        self.deadline.checked_duration_since(Instant::now())
    }

    /// Remaining whole-millisecond allowance, rounded up to keep sub-millisecond time usable.
    pub fn remaining_ms(self) -> Option<u64> {
        let remaining = self.remaining()?;
        if remaining.is_zero() {
            return None;
        }
        let millis = remaining.as_millis().try_into().unwrap_or(u64::MAX);
        Some(millis.max(1))
    }

    /// Whether the configured deadline has elapsed.
    pub fn is_expired(self) -> bool {
        Instant::now() >= self.deadline
    }
}

/// Input-event pacing constants.
#[derive(Debug, Clone, Copy)]
pub struct InputDelays {
    /// Polling interval for checking conditions.
    pub poll_interval_ms: u64,
    /// Delay between retry attempts.
    pub retry_delay_ms: u64,
}

/// Default input-event timings.
pub const INPUT_DELAYS: InputDelays = InputDelays {
    poll_interval_ms: 5,
    retry_delay_ms: 30,
};

/// Connection retry tuning.
#[derive(Debug, Clone, Copy)]
pub struct Retry {
    /// Fast retry delay after initial attempts.
    pub fast_delay_ms: u64,
}

/// Default connection retry pacing.
pub const RETRY: Retry = Retry { fast_delay_ms: 50 };

/// Warn overlay tuning.
#[derive(Debug, Clone, Copy)]
pub struct WarnOverlayConfig {
    /// Initial delay before starting tests after showing the hands-off overlay.
    pub initial_delay_ms: u64,
    /// Default size for the hands-off overlay window.
    pub width_px: f64,
    /// Default height of the warning overlay window.
    pub height_px: f64,
}

/// Default warning overlay geometry and timing.
pub const WARN_OVERLAY: WarnOverlayConfig = WarnOverlayConfig {
    initial_delay_ms: 0,
    width_px: 520.0,
    height_px: 140.0,
};

/// Convert milliseconds to `Duration`.
pub const fn ms(millis: u64) -> Duration {
    Duration::from_millis(millis)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exhausted_budget_has_no_remaining_milliseconds() {
        let now = Instant::now();
        let budget = RunBudget {
            started_at: now,
            deadline: now,
            total_ms: 1,
        };

        assert_eq!(budget.remaining_ms(), None);
        assert!(budget.is_expired());
    }
}
