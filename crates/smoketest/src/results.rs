//! Common test result types and utilities for all smoketest modules.

use std::{fmt, time::Duration};

/// Common test outcome data used across different test types.
#[derive(Debug, Clone, Default)]
pub struct TestOutcome {
    /// Whether the test passed
    pub success: bool,
    /// Time taken to complete the test
    pub elapsed: Duration,
    /// Test-specific details
    pub details: TestDetails,
    /// Optional message or description
    pub message: Option<String>,
}

/// Test-specific details that vary by test type.
#[derive(Debug, Clone, Default)]
pub enum TestDetails {
    /// UI test details
    Ui {
        /// Whether the HUD overlay was observed
        hud_seen: bool,
        /// Milliseconds from start until HUD became visible
        time_to_hud_ms: Option<u64>,
    },
    /// Focus test details
    /// Focus test results: observed title and pid
    Focus { title: String, pid: i32 },
    /// Repeat test details
    /// Repeat test results: number of repeats and type
    Repeat { count: usize, test_type: RepeatType },
    /// Window operation test details
    /// Window operation performed
    Window { operation: WindowOperation },
    /// Generic test with no specific details
    #[default]
    Generic,
}

/// Types of repeat tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepeatType {
    /// Relay repeats
    Relay,
    /// Shell repeats
    Shell,
    /// Volume repeats
    Volume,
}

impl fmt::Display for RepeatType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Relay => write!(f, "relay"),
            Self::Shell => write!(f, "shell"),
            Self::Volume => write!(f, "volume"),
        }
    }
}

/// Types of window operations tested.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowOperation {
    /// Raise a window
    Raise,
    /// Focus a window
    Focus,
    /// Hide or reveal a window
    Hide,
}

impl fmt::Display for WindowOperation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Raise => write!(f, "raise"),
            Self::Focus => write!(f, "focus"),
            Self::Hide => write!(f, "hide"),
        }
    }
}

impl TestOutcome {
    /// Create a successful test outcome.
    pub fn success(details: TestDetails) -> Self {
        Self {
            success: true,
            elapsed: Duration::default(),
            details,
            message: None,
        }
    }

    /// Create a failed test outcome.
    pub fn failure(details: TestDetails, message: impl Into<String>) -> Self {
        Self {
            success: false,
            elapsed: Duration::default(),
            details,
            message: Some(message.into()),
        }
    }

    /// Set the elapsed time for this outcome.
    pub fn with_elapsed(mut self, elapsed: Duration) -> Self {
        self.elapsed = elapsed;
        self
    }

    /// Set the elapsed time in milliseconds.
    pub fn with_elapsed_ms(mut self, ms: u64) -> Self {
        self.elapsed = Duration::from_millis(ms);
        self
    }

    /// Add a message to this outcome.
    pub fn with_message(mut self, message: impl Into<String>) -> Self {
        self.message = Some(message.into());
        self
    }

    /// Format the outcome as a status line for display.
    pub fn format_status(&self, test_name: &str) -> String {
        let status = if self.success { "OK" } else { "FAIL" };
        let elapsed = format!("{}ms", self.elapsed.as_millis());

        match &self.details {
            TestDetails::Ui {
                hud_seen,
                time_to_hud_ms,
            } => {
                format!(
                    "{}: {} (hud_seen={}, time_to_hud_ms={:?}, elapsed={})",
                    test_name, status, hud_seen, time_to_hud_ms, elapsed
                )
            }
            TestDetails::Focus { title, pid } => {
                format!(
                    "{}: {} (title='{}', pid={}, elapsed={})",
                    test_name, status, title, pid, elapsed
                )
            }
            TestDetails::Repeat { count, test_type } => {
                format!(
                    "{}: {} ({} {} repeats, elapsed={})",
                    test_name, status, count, test_type, elapsed
                )
            }
            TestDetails::Window { operation } => {
                format!(
                    "{}: {} ({} operation completed, elapsed={})",
                    test_name, status, operation, elapsed
                )
            }
            TestDetails::Generic => {
                if let Some(msg) = &self.message {
                    format!("{}: {} ({}, elapsed={})", test_name, status, msg, elapsed)
                } else {
                    format!("{}: {} (elapsed={})", test_name, status, elapsed)
                }
            }
        }
    }
}

/// Legacy Summary struct for backward compatibility.
/// Will be phased out in favor of TestOutcome.
#[derive(Debug, Clone)]
pub struct Summary {
    /// Whether HUD was seen during the test
    pub hud_seen: bool,
    /// Time to HUD visibility in milliseconds
    pub time_to_hud_ms: Option<u64>,
}

impl Default for Summary {
    fn default() -> Self {
        Self::new()
    }
}

impl Summary {
    pub fn new() -> Self {
        Self {
            hud_seen: false,
            time_to_hud_ms: None,
        }
    }

    /// Convert to the new TestOutcome format.
    pub fn to_outcome(self) -> TestOutcome {
        TestOutcome::success(TestDetails::Ui {
            hud_seen: self.hud_seen,
            time_to_hud_ms: self.time_to_hud_ms,
        })
        .with_elapsed_ms(self.time_to_hud_ms.unwrap_or(0))
    }
}

/// Legacy FocusOutcome struct for backward compatibility.
/// Will be phased out in favor of TestOutcome.
#[derive(Debug, Clone)]
pub struct FocusOutcome {
    /// Final focused window title
    pub title: String,
    /// Final focused window pid
    pub pid: i32,
    /// Elapsed time in milliseconds
    pub elapsed_ms: u64,
}

impl FocusOutcome {
    /// Convert to the new TestOutcome format.
    pub fn to_outcome(self) -> TestOutcome {
        TestOutcome::success(TestDetails::Focus {
            title: self.title,
            pid: self.pid,
        })
        .with_elapsed_ms(self.elapsed_ms)
    }
}
