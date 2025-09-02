use std::{
    io::Error as IoError,
    path::PathBuf,
    process::{Child, Command},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use tokio::time::{Instant, sleep};
use tracing::{debug, info, warn};

use crate::{Error, Result};

/// Time to wait for graceful shutdown after SIGTERM before escalating.
const TERM_WAIT_TIMEOUT_MS: u64 = 300;
/// Poll interval while waiting for graceful exit.
const TERM_POLL_INTERVAL_MS: u64 = 10;

#[inline]
fn send_sigterm(pid: libc::pid_t) {
    unsafe {
        libc::kill(pid, libc::SIGTERM);
    }
}

async fn wait_exit_async(child: &mut Child, timeout_ms: u64) -> bool {
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    while Instant::now() < deadline {
        match child.try_wait() {
            Ok(Some(_status)) => return true,
            Ok(None) => sleep(Duration::from_millis(TERM_POLL_INTERVAL_MS)).await,
            Err(_) => break,
        }
    }
    false
}

fn wait_exit_sync(child: &mut Child, timeout_ms: u64) -> bool {
    let start = std::time::Instant::now();
    while start.elapsed() < Duration::from_millis(timeout_ms) {
        if let Ok(Some(_)) = child.try_wait() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(TERM_POLL_INTERVAL_MS));
    }
    false
}

async fn terminate_child_async(child: &mut Child) -> Result<()> {
    let pid = child.id() as libc::pid_t;
    send_sigterm(pid);
    if wait_exit_async(child, TERM_WAIT_TIMEOUT_MS).await {
        info!("Server process exited gracefully");
        return Ok(());
    }
    warn!("Graceful stop timed out; escalating to SIGKILL");
    child.kill().map_err(Error::Io)?;
    match child.wait() {
        Ok(status) => info!("Server process killed: {:?}", status),
        Err(e) => warn!("Failed to wait for killed process: {}", e),
    }
    Ok(())
}

fn terminate_child_sync(mut child: Child) {
    let pid = child.id() as libc::pid_t;
    send_sigterm(pid);
    if wait_exit_sync(&mut child, TERM_WAIT_TIMEOUT_MS) {
        return;
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// Configuration for launching a hotkey server process
#[derive(Debug, Clone)]
pub(crate) struct ProcessConfig {
    /// Path to the executable
    pub executable: PathBuf,
    /// Arguments to pass to the server
    pub args: Vec<String>,
    /// Environment variables to set
    pub env: Vec<(String, String)>,
    /// Whether to inherit the parent's environment
    pub inherit_env: bool,
}

impl ProcessConfig {
    /// Create a new process configuration with the given executable
    pub fn new(executable: impl Into<PathBuf>) -> Self {
        Self {
            executable: executable.into(),
            args: vec!["--server".to_string()],
            env: Vec::new(),
            inherit_env: true,
        }
    }
}

/// A managed server process for hotkey handling
pub struct ServerProcess {
    child: Option<Child>,
    config: ProcessConfig,
    is_running: Arc<AtomicBool>,
}

impl ServerProcess {
    /// Create a new server process with the given configuration
    pub(crate) fn new(config: ProcessConfig) -> Self {
        Self {
            child: None,
            config,
            is_running: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Start the server process
    pub(crate) async fn start(&mut self) -> Result<()> {
        if self.is_running() {
            return Err(Error::HotkeyOperation(
                "Server is already running".to_string(),
            ));
        }

        info!("Starting server process: {:?}", self.config.executable);
        debug!("Server args: {:?}", self.config.args);

        let mut command = Command::new(&self.config.executable);

        // Add arguments
        for arg in &self.config.args {
            command.arg(arg);
        }

        // Configure environment
        if !self.config.inherit_env {
            command.env_clear();
        }

        for (key, value) in &self.config.env {
            command.env(key, value);
        }

        // Spawn the process
        let child = command.spawn().map_err(Error::Io)?;

        let pid = child.id();
        info!("Server process spawned with PID: {}", pid);

        self.child = Some(child);
        self.is_running.store(true, Ordering::SeqCst);

        // Wait for startup
        // Do not sleep here. Startup readiness is handled by client-side
        // connection polling to avoid duplicated timings.

        // Check if process is still running promptly
        if !self.is_running() {
            return Err(Error::HotkeyOperation(
                "Server process died during startup".to_string(),
            ));
        }

        Ok(())
    }

    /// Stop the server process
    pub(crate) async fn stop(&mut self) -> Result<()> {
        if let Some(mut child) = self.child.take() {
            info!("Stopping server process");
            terminate_child_async(&mut child).await?;
            self.is_running.store(false, Ordering::SeqCst);
        }

        Ok(())
    }

    /// Check if the server process is running
    pub(crate) fn is_running(&mut self) -> bool {
        if let Some(child) = self.child.as_mut() {
            // First, try to reap without blocking to update state promptly.
            if let Ok(Some(_status)) = child.try_wait() {
                self.is_running.store(false, Ordering::SeqCst);
                return false;
            }

            // Probe liveness via libc::kill(pid, 0) to avoid spawning processes.
            let pid = child.id() as libc::pid_t;
            let alive = unsafe { libc::kill(pid, 0) };
            // kill(0) returns 0 if the process exists and we have permission,
            // -1 with EPERM if it exists but we lack permission, and -1 with ESRCH if not.
            let running =
                alive == 0 || IoError::last_os_error().raw_os_error() == Some(libc::EPERM);
            self.is_running.store(running, Ordering::SeqCst);
            running
        } else {
            false
        }
    }

    /// Get the process ID if running
    pub fn pid(&self) -> Option<u32> {
        self.child.as_ref().map(|c| c.id())
    }
}

impl Drop for ServerProcess {
    fn drop(&mut self) {
        if self.is_running() {
            debug!("ServerProcess dropped while still running, attempting to stop");
            if let Some(child) = self.child.take() {
                terminate_child_sync(child);
                self.is_running.store(false, Ordering::SeqCst);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_process_config() {
        let config = ProcessConfig::new("/usr/bin/test");

        assert_eq!(config.executable, PathBuf::from("/usr/bin/test"));
        assert_eq!(config.args, vec!["--server"]);
        assert_eq!(config.env, Vec::<(String, String)>::new());
        assert!(config.inherit_env);
    }
}
