//! Deadline accounting for synchronous driver waits.

use std::time::{Duration, Instant};

/// Monotonic deadline with original timeout and elapsed-time helpers.
#[derive(Debug, Clone, Copy)]
pub(super) struct Deadline {
    /// Instant when the wait began.
    start: Instant,
    /// Instant when the wait expires.
    end: Instant,
    /// Original timeout duration in milliseconds.
    timeout_ms: u64,
}

impl Deadline {
    /// Build a deadline from a timeout in milliseconds.
    pub(super) fn from_timeout(timeout_ms: u64) -> Self {
        let start = Instant::now();
        Self {
            start,
            end: start + Duration::from_millis(timeout_ms),
            timeout_ms,
        }
    }

    /// Original timeout duration in milliseconds.
    pub(super) fn timeout_ms(self) -> u64 {
        self.timeout_ms
    }

    /// Elapsed time in milliseconds since the deadline was created.
    pub(super) fn elapsed_ms(self) -> u64 {
        self.start
            .elapsed()
            .as_millis()
            .try_into()
            .unwrap_or(u64::MAX)
    }

    /// Remaining duration until expiration.
    pub(super) fn remaining(self) -> Option<Duration> {
        self.end.checked_duration_since(Instant::now())
    }

    /// Remaining duration in milliseconds until expiration.
    pub(super) fn remaining_ms(self) -> Option<u64> {
        self.remaining()
            .map(|remaining| remaining.as_millis().try_into().unwrap_or(u64::MAX))
            .filter(|remaining| *remaining > 0)
    }

    /// Return true once the deadline has expired.
    pub(super) fn expired(self) -> bool {
        Instant::now() >= self.end
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deadline_reports_timeout_and_remaining_budget() {
        let deadline = Deadline::from_timeout(100);

        assert_eq!(deadline.timeout_ms(), 100);
        assert!(
            deadline
                .remaining_ms()
                .is_some_and(|remaining| remaining <= 100)
        );
        assert!(!deadline.expired());
    }
}
