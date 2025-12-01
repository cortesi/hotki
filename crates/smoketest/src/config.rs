//! Configuration constants and defaults for smoketests.

use std::{
    process,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

/// Default test-wide tunables.
#[derive(Debug, Clone, Copy)]
pub struct Defaults {
    /// Default duration for repeat tests in milliseconds.
    pub duration_ms: u64,
    /// Default timeout for UI readiness and waits in milliseconds.
    pub timeout_ms: u64,
    /// Minimum duration for volume tests to reduce flakiness.
    pub min_volume_duration_ms: u64,
}

/// Default test duration and timeout settings.
pub const DEFAULTS: Defaults = Defaults {
    duration_ms: 1000,
    timeout_ms: 3000,
    min_volume_duration_ms: 2000,
};

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
    poll_interval_ms: 10,
    retry_delay_ms: 80,
};

/// Connection retry tuning.
#[derive(Debug, Clone, Copy)]
pub struct Retry {
    /// Fast retry delay after initial attempts.
    pub fast_delay_ms: u64,
}

/// Default connection retry pacing.
pub const RETRY: Retry = Retry { fast_delay_ms: 50 };

/// Bridge-level handshake timing configuration.
#[derive(Debug, Clone, Copy)]
pub struct BridgeConfig {
    /// Maximum time the UI may take to acknowledge a command before we fail fast.
    pub ack_timeout_ms: u64,
}

/// Default bridge handshake thresholds.
pub const BRIDGE: BridgeConfig = BridgeConfig {
    ack_timeout_ms: 750,
};

/// Canonical RPC binding gate timings.
#[derive(Debug, Clone, Copy)]
pub struct BindingGates {
    /// Default binding readiness gate for non-raise tests (RPC mode).
    pub default_ms: u64,
}

/// Default RPC readiness gate tunables.
pub const BINDING_GATES: BindingGates = BindingGates { default_ms: 2000 };

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
    initial_delay_ms: 2000,
    width_px: 520.0,
    height_px: 140.0,
};

/// Generate a unique window title with a simple prefix.
pub fn test_title(prefix: &str) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("hotki smoketest: {} {}-{}", prefix, process::id(), now)
}

/// Convert milliseconds to `Duration`.
pub const fn ms(millis: u64) -> Duration {
    Duration::from_millis(millis)
}
