//! Configuration constants and defaults for smoketests.

use std::time::Duration;

/// Default test-wide tunables.
#[derive(Debug, Clone, Copy)]
pub struct Defaults {
    /// Default timeout for UI readiness and waits in milliseconds.
    pub timeout_ms: u64,
}

/// Default timeout settings.
pub const DEFAULTS: Defaults = Defaults { timeout_ms: 3000 };

/// Delay between theme switches to make transitions visible (milliseconds).
pub const THEME_SWITCH_DELAY_MS: u64 = 150;

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
pub const BINDING_GATES: BindingGates = BindingGates { default_ms: 500 };

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
