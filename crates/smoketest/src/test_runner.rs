//! Common test infrastructure and execution patterns for smoketests.

use std::{
    env, fs,
    path::PathBuf,
    process,
    time::{Duration, Instant},
};

use crate::{
    config,
    error::{Error, Result},
    server_drive,
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
            config_path: None,
            temp_config: None,
        }
    }

    /// Enable logging for this test.
    pub fn with_logs(mut self, enabled: bool) -> Self {
        self.with_logs = enabled;
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

    /// Get remaining time before timeout.
    pub fn remaining_ms(&self) -> u64 {
        self.config.timeout_ms.saturating_sub(self.elapsed_ms())
    }

    /// Launch hotki with the configured settings.
    pub fn launch_hotki(&mut self) -> Result<()> {
        let hotki_bin = resolve_hotki_bin().ok_or(Error::HotkiBinNotFound)?;

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
            let cwd = env::current_dir()?;
            let path = cwd.join(config::DEFAULT_TEST_CONFIG_PATH);
            if !path.exists() {
                return Err(Error::MissingConfig(path));
            }
            path
        };

        // Launch session
        let session =
            HotkiSession::launch_with_config(&hotki_bin, &config_path, self.config.with_logs)?;
        self.session = Some(session);
        Ok(())
    }

    /// Wait for HUD to become visible.
    pub fn wait_for_hud(&mut self) -> Result<u64> {
        let remaining = self.remaining_ms();
        let timeout = self.config.timeout_ms;

        let session = self
            .session
            .as_mut()
            .ok_or_else(|| Error::InvalidState("No session launched".into()))?;

        match session.wait_for_hud_checked(remaining) {
            Ok(ms) => Ok(ms),
            Err(Error::HudNotVisible { .. }) => Err(Error::HudNotVisible {
                timeout_ms: timeout,
            }),
            Err(e) => Err(e),
        }
    }

    /// Shutdown the hotki session.
    pub fn shutdown(&mut self) {
        if let Some(mut session) = self.session.take() {
            session.shutdown();
            session.kill_and_wait();
        }
        // Ensure any shared RPC connection from a prior test is cleared.
        server_drive::reset();
    }

    /// Ensure the MRPC driver is initialized and required idents are registered.
    pub fn ensure_rpc_ready(&self, idents: &[&str]) -> Result<()> {
        let sock = self
            .session
            .as_ref()
            .ok_or_else(|| Error::InvalidState("session not launched".into()))?
            .socket_path()
            .to_string();

        server_drive::ensure_init(&sock, 3000)?;
        for ident in idents {
            server_drive::wait_for_ident(ident, config::BINDING_GATE_DEFAULT_MS)?;
        }
        Ok(())
    }

    /// Clean up all temporary files.
    fn cleanup_temp_files(&mut self) {
        for path in self.temp_files.drain(..) {
            if let Err(_e) = fs::remove_file(path) {}
        }
    }
}

impl Drop for TestContext {
    fn drop(&mut self) {
        self.shutdown();
        self.cleanup_temp_files();
    }
}

// Type aliases to reduce complexity
/// Setup phase callback signature.
type SetupFn = Box<dyn FnOnce(&mut TestContext) -> Result<()>>;
/// Execute phase callback signature.
type ExecuteFn<T> = Box<dyn FnOnce(&mut TestContext) -> Result<T>>;
/// Teardown phase callback signature.
type TeardownFn<T> = Box<dyn FnOnce(&mut TestContext, &T) -> Result<()>>;

/// Builder pattern for test execution.
pub struct TestRunner<T> {
    /// Test configuration used by this runner.
    config: TestConfig,
    /// Optional setup step executed before the test.
    setup: Option<SetupFn>,
    /// Main test logic producing a value `T`.
    execute: Option<ExecuteFn<T>>,
    /// Optional teardown step executed after success.
    teardown: Option<TeardownFn<T>>,
}

impl<T> TestRunner<T> {
    /// Create a new test runner.
    pub fn new(_name: impl Into<String>, config: TestConfig) -> Self {
        Self {
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

/// Create a temporary config file and return its path.
pub fn create_temp_config(content: &str) -> Result<PathBuf> {
    use std::time::{SystemTime, UNIX_EPOCH};

    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();

    let temp_path = env::temp_dir().join(format!(
        "hotki-smoketest-{}-{}.ron",
        process::id(),
        timestamp
    ));

    fs::write(&temp_path, content)?;
    Ok(temp_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ensure_rpc_ready_without_session_errors() {
        let config = TestConfig::new(1000);
        let ctx = TestContext::new(config);
        let err = ctx.ensure_rpc_ready(&["shift+cmd+0"]).unwrap_err();
        assert!(matches!(err, Error::InvalidState(_)));
    }
}
