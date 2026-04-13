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
const SERVER_FLAG: &str = "--server";
const SOCKET_FLAG: &str = "--socket";

pub(crate) const PARENT_PID_FLAG: &str = "--parent-pid";
pub(crate) const LOG_FILTER_FLAG: &str = "--log-filter";

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

fn replace_flag_pair(args: &mut Vec<String>, flag: &str, value: &str) {
    let mut new_args: Vec<String> = Vec::with_capacity(args.len() + 2);
    let mut i = 0;
    while i < args.len() {
        if args[i] == flag {
            i += 1;
            if i < args.len() {
                i += 1;
            }
        } else {
            new_args.push(args[i].clone());
            i += 1;
        }
    }
    new_args.push(flag.to_string());
    new_args.push(value.to_string());
    *args = new_args;
}

/// Configuration for launching a hotkey server process.
///
/// The server is spawned with the parent's unmodified environment. All
/// server-specific state (parent PID, log filter) is passed via CLI
/// arguments so nothing hotki wants the backend to know ever leaks into
/// grandchild processes such as shell actions.
#[derive(Debug, Clone)]
pub(crate) struct ProcessConfig {
    /// Path to the executable
    pub executable: PathBuf,
    /// Arguments to pass to the server
    pub args: Vec<String>,
}

impl ProcessConfig {
    /// Create a new process configuration with the given executable
    pub fn new(executable: impl Into<PathBuf>) -> Self {
        Self {
            executable: executable.into(),
            args: vec![SERVER_FLAG.to_string()],
        }
    }

    /// Ensure the process args contain a single `--server` flag.
    pub(crate) fn ensure_server_mode(&mut self) {
        if !self.args.iter().any(|arg| arg == SERVER_FLAG) {
            self.args.insert(0, SERVER_FLAG.to_string());
        }
    }

    /// Replace any existing `--socket <value>` pair with the supplied socket path.
    pub(crate) fn set_socket_path(&mut self, socket_path: &str) {
        replace_flag_pair(&mut self.args, SOCKET_FLAG, socket_path);
    }

    /// Replace any existing `--parent-pid <value>` pair with the supplied PID.
    pub(crate) fn set_parent_pid(&mut self, pid: u32) {
        replace_flag_pair(&mut self.args, PARENT_PID_FLAG, &pid.to_string());
    }

    /// Replace any existing `--log-filter <value>` pair with the supplied filter spec.
    pub(crate) fn set_log_filter(&mut self, filter: &str) {
        replace_flag_pair(&mut self.args, LOG_FILTER_FLAG, filter);
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

        debug!("Starting server process: {:?}", self.config.executable);
        debug!("Server args: {:?}", self.config.args);

        let mut command = Command::new(&self.config.executable);
        for arg in &self.config.args {
            command.arg(arg);
        }
        let child = command.spawn().map_err(Error::Io)?;

        let pid = child.id();
        debug!("Server process spawned with PID: {}", pid);

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
    }

    #[test]
    fn set_parent_pid_is_idempotent() {
        let mut config = ProcessConfig::new("/usr/bin/test");
        config.set_parent_pid(111);
        config.set_parent_pid(222);
        let count = config.args.iter().filter(|a| *a == PARENT_PID_FLAG).count();
        assert_eq!(count, 1);
        let idx = config
            .args
            .iter()
            .position(|a| a == PARENT_PID_FLAG)
            .unwrap();
        assert_eq!(config.args[idx + 1], "222");
    }

    #[test]
    fn set_log_filter_is_idempotent() {
        let mut config = ProcessConfig::new("/usr/bin/test");
        config.set_log_filter("a=info");
        config.set_log_filter("b=debug");
        let count = config.args.iter().filter(|a| *a == LOG_FILTER_FLAG).count();
        assert_eq!(count, 1);
        let idx = config
            .args
            .iter()
            .position(|a| a == LOG_FILTER_FLAG)
            .unwrap();
        assert_eq!(config.args[idx + 1], "b=debug");
    }
}
