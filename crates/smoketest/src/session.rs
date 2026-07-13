use std::{env, path::PathBuf, process::Command};

use logging as logshared;
use tracing::debug;

use crate::{
    config::RunBudget,
    error::{Error, Result},
    process::{self, ManagedChild},
    server_drive::ServerDriver,
};

/// Launch configuration for a smoketest-backed hotki app session.
pub struct HotkiSessionConfig {
    /// Path to the hotki app binary to run.
    binary_path: PathBuf,
    /// Optional path to a config file to load.
    config_path: Option<PathBuf>,
    /// Whether to enable verbose logs for the child.
    with_logs: bool,
}

impl HotkiSessionConfig {
    /// Construct a configuration using the default hotki app binary resolution.
    pub fn from_env() -> Result<Self> {
        Ok(Self {
            binary_path: resolve_hotki_app_binary()?,
            config_path: None,
            with_logs: false,
        })
    }

    /// Provide a configuration file path to the hotki process.
    #[must_use]
    pub fn with_config(mut self, path: impl Into<PathBuf>) -> Self {
        self.config_path = Some(path.into());
        self
    }

    /// Enable or disable child process logging via `RUST_LOG`.
    #[must_use]
    pub fn with_logs(mut self, enable: bool) -> Self {
        self.with_logs = enable;
        self
    }
}

/// Running hotki process with helpers for RPC and shutdown.
pub struct HotkiSession {
    /// Child process handle.
    child: ManagedChild,
    /// Driver handle used to communicate with the server.
    driver: ServerDriver,
    /// Whether teardown has already been performed.
    cleaned_up: bool,
}

impl HotkiSession {
    /// Spawn a hotki app process according to the supplied configuration.
    pub fn spawn(config: HotkiSessionConfig, run_budget: RunBudget) -> Result<Self> {
        let HotkiSessionConfig {
            binary_path,
            config_path,
            with_logs,
        } = config;
        let mut cmd = Command::new(&binary_path);
        cmd.arg("--disable-event-tap");
        if with_logs {
            cmd.env("RUST_LOG", logshared::log_config_for_child());
        }
        if let Some(cfg) = &config_path {
            cmd.arg("--config");
            cmd.arg(cfg);
        }

        let mut child = process::spawn_managed(cmd)?;
        let mut driver = ServerDriver::new(socket_path_for_pid(child.pid as u32))?;
        let readiness_ms = run_budget.remaining_ms().ok_or_else(|| {
            Error::InvalidState(format!(
                "run budget exhausted before server readiness ({} ms total)",
                run_budget.total_ms()
            ))
        })?;
        if let Err(err) = driver.ensure_ready(readiness_ms) {
            if let Err(kill_err) = child.kill_and_wait() {
                debug!(
                    ?kill_err,
                    "failed to terminate hotki after server driver init failure"
                );
            }
            return Err(Error::from(err));
        }
        Ok(Self {
            child,
            driver,
            cleaned_up: false,
        })
    }

    /// Return the OS process id for the hotki app child.
    pub fn pid(&self) -> u32 {
        self.child.pid as u32
    }

    /// Borrow the server driver mutably.
    pub fn driver_mut(&mut self) -> &mut ServerDriver {
        &mut self.driver
    }

    /// Attempt a graceful server shutdown via the driver, surfacing failures.
    pub fn shutdown(&mut self) -> Result<()> {
        if self.cleaned_up {
            return Ok(());
        }
        self.driver.shutdown().map_err(Error::from)
    }

    /// Forcefully kill the child process and wait for exit.
    pub fn kill_and_wait(&mut self) {
        if self.cleaned_up {
            return;
        }
        if let Err(_e) = self.child.kill_and_wait() {}
        self.driver.reset();
        self.cleaned_up = true;
    }
}

impl Drop for HotkiSession {
    fn drop(&mut self) {
        if self.cleaned_up {
            return;
        }
        if let Err(err) = self.shutdown() {
            debug!(?err, "server driver shutdown during drop failed");
        }
        self.kill_and_wait();
    }
}

// ===== Socket Path Management =====

/// Generate the socket path for a given process ID
pub fn socket_path_for_pid(pid: u32) -> String {
    hotki_server::socket_path_for_pid(pid)
}

/// Resolve the Cargo-built hotki app beside the current smoketest executable.
fn resolve_hotki_app_binary() -> Result<PathBuf> {
    let inferred = env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join("hotki-app")))
        .filter(|path| path.exists());

    inferred.ok_or(Error::HotkiAppBinNotFound)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::DEFAULT_RUN_BUDGET_MS;

    #[test]
    fn spawn_initializes_server_driver() -> Result<()> {
        let config = match HotkiSessionConfig::from_env() {
            Ok(cfg) => cfg.with_logs(false),
            Err(Error::HotkiAppBinNotFound) => return Ok(()),
            Err(other) => return Err(other),
        };
        let mut session = HotkiSession::spawn(config, RunBudget::new(DEFAULT_RUN_BUDGET_MS))?;

        session.driver_mut().check_alive()?;

        session.shutdown()?;
        session.kill_and_wait();
        Ok(())
    }
}
