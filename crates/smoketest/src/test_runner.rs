//! Common test infrastructure and execution patterns for smoketests.

use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::{
    config,
    error::{Error, Result},
    session::HotkiSession,
    util::resolve_hotki_bin,
};

/// Configuration for running a test.
#[derive(Debug, Clone)]
pub struct TestConfig {
    /// Maximum time to wait for test completion
    pub timeout_ms: u64,
    /// Whether to enable logging
    pub with_logs: bool,
    /// Test duration for repeat tests
    pub duration_ms: Option<u64>,
    /// Custom config path (if not using default)
    pub config_path: Option<PathBuf>,
    /// Temporary config content (will create temp file)
    pub temp_config: Option<String>,
}

impl TestConfig {
    /// Create a new test configuration with defaults.
    pub fn new(timeout_ms: u64) -> Self {
        Self {
            timeout_ms,
            with_logs: false,
            duration_ms: None,
            config_path: None,
            temp_config: None,
        }
    }

    /// Enable logging for this test.
    pub fn with_logs(mut self, enabled: bool) -> Self {
        self.with_logs = enabled;
        self
    }

    /// Set the duration for repeat tests.
    pub fn with_duration(mut self, ms: u64) -> Self {
        self.duration_ms = Some(ms);
        self
    }

    /// Use a specific config file.
    pub fn with_config_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.config_path = Some(path.into());
        self
    }

    /// Use temporary config content.
    pub fn with_temp_config(mut self, content: impl Into<String>) -> Self {
        self.temp_config = Some(content.into());
        self
    }
}

/// Common test context that manages lifecycle.
pub struct TestContext {
    /// Test configuration
    pub config: TestConfig,
    /// Hotki session (if launched)
    pub session: Option<HotkiSession>,
    /// Temporary files to clean up
    temp_files: Vec<PathBuf>,
    /// Test start time
    start_time: Instant,
}

impl TestContext {
    /// Create a new test context.
    pub fn new(config: TestConfig) -> Self {
        Self {
            config,
            session: None,
            temp_files: Vec::new(),
            start_time: Instant::now(),
        }
    }

    /// Get the elapsed time since test start.
    pub fn elapsed(&self) -> Duration {
        self.start_time.elapsed()
    }

    /// Get the elapsed time in milliseconds.
    pub fn elapsed_ms(&self) -> u64 {
        self.elapsed().as_millis() as u64
    }

    /// Check if timeout has been exceeded.
    pub fn is_timeout(&self) -> bool {
        self.elapsed_ms() > self.config.timeout_ms
    }

    /// Get remaining time before timeout.
    pub fn remaining_ms(&self) -> u64 {
        self.config.timeout_ms.saturating_sub(self.elapsed_ms())
    }

    /// Create a deadline for operations.
    pub fn deadline(&self) -> Instant {
        self.start_time + Duration::from_millis(self.config.timeout_ms)
    }

    /// Launch hotki with the configured settings.
    pub fn launch_hotki(&mut self) -> Result<()> {
        let hotki_bin = resolve_hotki_bin()
            .ok_or(Error::HotkiBinNotFound)?;

        // Determine config path
        let config_path = if let Some(content) = &self.config.temp_config {
            // Create temporary config file
            let temp_path = create_temp_config(content)?;
            self.temp_files.push(temp_path.clone());
            temp_path
        } else if let Some(path) = &self.config.config_path {
            path.clone()
        } else {
            // Use default test config
            let cwd = std::env::current_dir()?;
            let path = cwd.join(config::DEFAULT_TEST_CONFIG_PATH);
            if !path.exists() {
                return Err(Error::MissingConfig(path));
            }
            path
        };

        // Launch session
        let session = HotkiSession::launch_with_config(
            &hotki_bin,
            &config_path,
            self.config.with_logs,
        )?;
        self.session = Some(session);
        Ok(())
    }

    /// Wait for HUD to become visible.
    pub fn wait_for_hud(&mut self) -> Result<u64> {
        let remaining = self.remaining_ms();
        let timeout = self.config.timeout_ms;
        
        let session = self.session.as_mut()
            .ok_or_else(|| Error::InvalidState("No session launched".into()))?;
        
        let (seen, time_ms) = session.wait_for_hud(remaining);
        if !seen {
            return Err(Error::HudNotVisible {
                timeout_ms: timeout,
            });
        }
        Ok(time_ms)
    }

    /// Shutdown the hotki session.
    pub fn shutdown(&mut self) {
        if let Some(mut session) = self.session.take() {
            session.shutdown();
            session.kill_and_wait();
        }
    }

    /// Register a temporary file for cleanup.
    pub fn register_temp_file(&mut self, path: impl Into<PathBuf>) {
        self.temp_files.push(path.into());
    }

    /// Clean up all temporary files.
    fn cleanup_temp_files(&mut self) {
        for path in self.temp_files.drain(..) {
            let _ = fs::remove_file(path);
        }
    }
}

impl Drop for TestContext {
    fn drop(&mut self) {
        self.shutdown();
        self.cleanup_temp_files();
    }
}

/// Builder pattern for test execution.
pub struct TestRunner<T> {
    name: String,
    config: TestConfig,
    setup: Option<Box<dyn FnOnce(&mut TestContext) -> Result<()>>>,
    execute: Option<Box<dyn FnOnce(&mut TestContext) -> Result<T>>>,
    teardown: Option<Box<dyn FnOnce(&mut TestContext, &T) -> Result<()>>>,
}

impl<T> TestRunner<T> {
    /// Create a new test runner.
    pub fn new(name: impl Into<String>, config: TestConfig) -> Self {
        Self {
            name: name.into(),
            config,
            setup: None,
            execute: None,
            teardown: None,
        }
    }

    /// Set the setup phase.
    pub fn with_setup<F>(mut self, f: F) -> Self
    where
        F: FnOnce(&mut TestContext) -> Result<()> + 'static,
    {
        self.setup = Some(Box::new(f));
        self
    }

    /// Set the execution phase.
    pub fn with_execute<F>(mut self, f: F) -> Self
    where
        F: FnOnce(&mut TestContext) -> Result<T> + 'static,
    {
        self.execute = Some(Box::new(f));
        self
    }

    /// Set the teardown phase.
    pub fn with_teardown<F>(mut self, f: F) -> Self
    where
        F: FnOnce(&mut TestContext, &T) -> Result<()> + 'static,
    {
        self.teardown = Some(Box::new(f));
        self
    }

    /// Run the test.
    pub fn run(self) -> Result<T> {
        let mut context = TestContext::new(self.config);
        
        // Setup phase
        if let Some(setup) = self.setup {
            setup(&mut context)?;
        }

        // Execute phase
        let result = if let Some(execute) = self.execute {
            execute(&mut context)?
        } else {
            return Err(Error::InvalidState("No execute phase defined".into()));
        };

        // Teardown phase
        if let Some(teardown) = self.teardown {
            teardown(&mut context, &result)?;
        }

        Ok(result)
    }
}

/// Standard test pattern: launch hotki, wait for HUD, execute test
pub fn run_standard_test<F, T>(
    name: &str,
    config: TestConfig,
    test_fn: F,
) -> Result<T>
where
    F: FnOnce(&mut TestContext) -> Result<T> + 'static,
    T: 'static,
{
    TestRunner::new(name, config)
        .with_setup(|ctx| {
            ctx.launch_hotki()?;
            ctx.wait_for_hud()?;
            Ok(())
        })
        .with_execute(test_fn)
        .with_teardown(|ctx, _| {
            ctx.shutdown();
            Ok(())
        })
        .run()
}

/// Create a temporary config file and return its path.
pub fn create_temp_config(content: &str) -> Result<PathBuf> {
    use std::time::{SystemTime, UNIX_EPOCH};
    
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    
    let temp_path = std::env::temp_dir().join(format!(
        "hotki-smoketest-{}-{}.ron",
        std::process::id(),
        timestamp
    ));
    
    fs::write(&temp_path, content)?;
    Ok(temp_path)
}

/// Helper to wait for a condition with timeout.
pub fn wait_for<F>(
    condition: F,
    timeout_ms: u64,
    poll_interval_ms: u64,
) -> bool
where
    F: Fn() -> bool,
{
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    let poll_interval = Duration::from_millis(poll_interval_ms);
    
    while Instant::now() < deadline {
        if condition() {
            return true;
        }
        std::thread::sleep(poll_interval);
    }
    false
}

/// Helper to retry an operation with delays.
pub fn retry_with_delay<F, T>(
    mut operation: F,
    max_attempts: u32,
    delay_ms: u64,
) -> Option<T>
where
    F: FnMut() -> Option<T>,
{
    for _ in 0..max_attempts {
        if let Some(result) = operation() {
            return Some(result);
        }
        if delay_ms > 0 {
            std::thread::sleep(Duration::from_millis(delay_ms));
        }
    }
    None
}