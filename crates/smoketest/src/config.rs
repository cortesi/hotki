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
    timeout_ms: 10000,
    min_volume_duration_ms: 2000,
};

/// Input-event pacing constants.
#[derive(Debug, Clone, Copy)]
pub struct InputDelays {
    /// Polling interval for checking conditions.
    pub poll_interval_ms: u64,
    /// Short delay between key events.
    pub key_event_delay_ms: u64,
    /// Standard delay between UI actions.
    pub ui_action_delay_ms: u64,
    /// Delay between retry attempts.
    pub retry_delay_ms: u64,
}

/// Default input-event timings.
pub const INPUT_DELAYS: InputDelays = InputDelays {
    poll_interval_ms: 10,
    key_event_delay_ms: 25,
    ui_action_delay_ms: 50,
    retry_delay_ms: 80,
};

/// Wait-time bounds shared across tests.
#[derive(Debug, Clone, Copy)]
pub struct Waits {
    /// Timeout for waiting for the first window.
    pub first_window_ms: u64,
}

/// Standard wait budgets.
pub const WAITS: Waits = Waits {
    first_window_ms: 6000,
};

/// Connection retry tuning.
#[derive(Debug, Clone, Copy)]
pub struct Retry {
    /// Initial delay for connection retry (increases after first attempts).
    pub initial_delay_ms: u64,
    /// Fast retry delay after initial attempts.
    pub fast_delay_ms: u64,
    /// Number of initial connection attempts before switching to fast retry.
    pub initial_attempts: u32,
    /// Delay for waiting between event checks.
    pub event_check_interval_ms: u64,
}

/// Default connection retry pacing.
pub const RETRY: Retry = Retry {
    initial_delay_ms: 200,
    fast_delay_ms: 50,
    initial_attempts: 3,
    event_check_interval_ms: 80,
};

/// Helper window defaults shared across tests.
#[derive(Debug, Clone, Copy)]
pub struct HelperWindowConfig {
    /// Default time to keep helper windows alive in milliseconds.
    pub default_lifetime_ms: u64,
    /// Extra time to add to helper window lifetime.
    pub extra_time_ms: u64,
    /// Default helper window width in pixels for test helpers.
    pub width_px: f64,
    /// Default helper window height in pixels for test helpers.
    pub height_px: f64,
    /// Margin from screen edge when placing helper windows.
    pub margin_px: f64,
}

/// Helper window geometry and lifetime defaults.
pub const HELPER_WINDOW: HelperWindowConfig = HelperWindowConfig {
    default_lifetime_ms: 15000,
    extra_time_ms: 5000,
    width_px: 280.0,
    height_px: 180.0,
    margin_px: 8.0,
};

/// Canonical RPC binding gate timings.
#[derive(Debug, Clone, Copy)]
pub struct BindingGates {
    /// Default binding readiness gate for non-raise tests (RPC mode).
    pub default_ms: u64,
}

/// Default RPC readiness gate tunables.
pub const BINDING_GATES: BindingGates = BindingGates { default_ms: 2000 };

/// Placement test tunables.
#[derive(Debug, Clone, Copy)]
pub struct PlaceConfig {
    /// Default number of columns for placement grid.
    pub grid_cols: u32,
    /// Default number of rows for placement grid.
    pub grid_rows: u32,
    /// Epsilon in pixels for frame comparisons in placement checks.
    pub eps: f64,
    /// Per-cell timeout while waiting for the expected frame.
    pub step_timeout_ms: u64,
}

/// Default placement grid tuning.
pub const PLACE: PlaceConfig = PlaceConfig {
    grid_cols: 3,
    grid_rows: 2,
    eps: 2.0,
    step_timeout_ms: 3000,
};

/// Session-level defaults.
#[derive(Debug, Clone, Copy)]
pub struct SessionConfig {
    /// Interval to resend activation chord while waiting for HUD.
    pub activation_resend_interval_ms: u64,
}

/// Default session pacing.
pub const SESSION: SessionConfig = SessionConfig {
    activation_resend_interval_ms: 500,
};

/// Repository-relative configuration paths.
#[derive(Debug, Clone, Copy)]
pub struct Paths {
    /// Default test configuration file path relative to repo root.
    pub test_config: &'static str,
}

/// Default smoketest configuration paths.
pub const PATHS: Paths = Paths {
    test_config: "examples/test.ron",
};

/// Titles used for helper windows.
#[derive(Debug, Clone, Copy)]
pub struct Titles {
    /// Base title for relay repeat test window.
    pub relay_test: &'static str,
}

/// Default helper window titles.
pub const TITLES: Titles = Titles {
    relay_test: "hotki smoketest: relayrepeat",
};

/// Fullscreen test timing.
#[derive(Debug, Clone, Copy)]
pub struct FullscreenConfig {
    /// Delay after launching the fullscreen helper before measuring.
    pub helper_show_delay_ms: u64,
    /// Maximum time to wait for fullscreen frame stabilization.
    pub wait_total_ms: u64,
    /// Poll interval while waiting for fullscreen frame changes.
    pub wait_poll_ms: u64,
}

/// Default fullscreen timing.
pub const FULLSCREEN: FullscreenConfig = FullscreenConfig {
    helper_show_delay_ms: 300,
    wait_total_ms: 1000,
    wait_poll_ms: 50,
};

/// Hide test tunables.
#[derive(Debug, Clone, Copy)]
pub struct HideConfig {
    /// Extra time for hide test helper windows.
    pub helper_extra_time_ms: u64,
    /// Offset for window positioning tests.
    pub position_offset: f64,
    /// Minimum timeout for hide operations (1/4 of main timeout).
    pub min_timeout_ms: u64,
    /// Minimum timeout for secondary hide operations (1/3 of main timeout).
    pub secondary_min_timeout_ms: u64,
    /// Max wait for the helper window to appear initially.
    pub first_window_max_ms: u64,
    /// Poll interval for hide position/frame checks.
    pub poll_ms: u64,
    /// Delay after activation before sending next hide keys.
    pub activate_post_delay_ms: u64,
    /// Max wait for the window to restore frame on hide(off).
    pub restore_max_ms: u64,
}

/// Default hide test timing and geometry.
pub const HIDE: HideConfig = HideConfig {
    helper_extra_time_ms: 8000,
    position_offset: 300.0,
    min_timeout_ms: 800,
    secondary_min_timeout_ms: 1000,
    first_window_max_ms: 2000,
    poll_ms: 50,
    activate_post_delay_ms: 100,
    restore_max_ms: 1200,
};

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
