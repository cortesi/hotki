use std::{
    env, fs,
    io::{self, BufRead, BufReader, Write},
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    process::{self as std_process, Command},
    sync::atomic::{AtomicU64, Ordering},
};

use hotki_protocol::NotifyKind;
use logging as logshared;
use socket2::{Domain, SockAddr, Socket, Type};
use tracing::debug;

use crate::{
    config::RunBudget,
    error::{Error, Result},
    process::{self, ManagedChild},
    server_drive::ServerDriver,
    windows::OwnedWindows,
};

/// Monotonic suffix for process-local control socket names.
static NEXT_CONTROL_SOCKET: AtomicU64 = AtomicU64::new(0);

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
    /// App-owned local control socket used for graceful UI shutdown.
    control_socket: PathBuf,
    /// Shared monotonic budget for readiness, presentation, and teardown.
    run_budget: RunBudget,
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
        let control_socket = next_control_socket_path()?;
        cmd.arg("--harness-control-socket");
        cmd.arg(&control_socket);
        if with_logs {
            cmd.env("RUST_LOG", logshared::log_config_for_child());
        }
        if let Some(cfg) = &config_path {
            cmd.arg("--config");
            cmd.arg(cfg);
        }

        let mut child = process::spawn_managed(cmd)?;
        let driver_result = (|| -> Result<ServerDriver> {
            let mut driver = ServerDriver::new(socket_path_for_pid(child.pid as u32))?;
            let readiness_ms = run_budget.remaining_ms().ok_or_else(|| {
                Error::InvalidState(format!(
                    "run budget exhausted before server readiness ({} ms total)",
                    run_budget.total_ms()
                ))
            })?;
            driver.ensure_ready(readiness_ms)?;
            Ok(driver)
        })();
        let driver = match driver_result {
            Ok(driver) => driver,
            Err(err) => {
                if let Err(kill_err) = child.kill_and_wait() {
                    debug!(
                        ?kill_err,
                        "failed to terminate hotki after server driver init failure"
                    );
                }
                remove_control_socket(&control_socket);
                return Err(err);
            }
        };
        Ok(Self {
            child,
            driver,
            control_socket,
            run_budget,
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

    /// Return PID-scoped native window operations for this app process.
    pub fn windows(&self) -> OwnedWindows {
        OwnedWindows::new(self.pid())
    }

    /// Wait until the app confirms that a visible HUD state survived a rendered frame.
    pub fn wait_for_hud_frame(&self) -> Result<()> {
        self.request_control("present hud")
    }

    /// Wait until the app confirms that this selector query survived a rendered frame.
    pub fn wait_for_selector_frame(&self, query: &str) -> Result<()> {
        self.request_control(&format!("present selector {query}"))
    }

    /// Wait until the app confirms that this notification kind survived a rendered frame.
    pub fn wait_for_notification_frame(&self, kind: NotifyKind) -> Result<()> {
        let kind = match kind {
            NotifyKind::Info => "info",
            NotifyKind::Warn => "warn",
            NotifyKind::Error => "error",
            NotifyKind::Success => "success",
            NotifyKind::Ignore => {
                return Err(Error::InvalidState(
                    "ignored notifications have no visible presentation".to_string(),
                ));
            }
        };
        self.request_control(&format!("present notification {kind}"))
    }

    /// Request graceful app-local shutdown and wait for the owned process to exit.
    pub fn shutdown(&mut self) -> Result<()> {
        if self.cleaned_up {
            return Ok(());
        }
        if let Err(error) = self.request_control("shutdown") {
            self.kill_and_wait();
            return Err(error);
        }
        let Some(remaining) = self.run_budget.remaining() else {
            self.kill_and_wait();
            return Err(self.budget_exhausted("graceful app exit"));
        };
        let wait_result = self.child.wait_with_budget(remaining);
        self.finish_cleanup();
        let (status, forced) = wait_result?;
        if forced {
            return Err(self.budget_exhausted("graceful app exit"));
        }
        if !status.success() {
            return Err(Error::InvalidState(format!(
                "hotki app exited unsuccessfully after shutdown request: {status}"
            )));
        }
        Ok(())
    }

    /// Send one private app-control command within the session's remaining budget.
    fn request_control(&self, command: &str) -> Result<()> {
        let Some(remaining) = self.run_budget.remaining() else {
            return Err(self.budget_exhausted(command));
        };
        let socket = Socket::new(Domain::UNIX, Type::STREAM, None)?;
        let address = SockAddr::unix(&self.control_socket)?;
        socket.connect_timeout(&address, remaining)?;
        let mut stream = UnixStream::from(socket);
        let Some(remaining) = self.run_budget.remaining() else {
            return Err(self.budget_exhausted(command));
        };
        stream.set_write_timeout(Some(remaining))?;
        stream.write_all(command.as_bytes())?;
        stream.write_all(b"\n")?;
        let Some(remaining) = self.run_budget.remaining() else {
            return Err(self.budget_exhausted(command));
        };
        stream.set_read_timeout(Some(remaining))?;
        let mut response = String::new();
        BufReader::new(stream).read_line(&mut response)?;
        if response != "ok\n" {
            return Err(Error::InvalidState(format!(
                "app rejected harness command '{command}' at {}: {}",
                self.control_socket.display(),
                response.trim_end()
            )));
        }
        Ok(())
    }

    /// Describe exhaustion of the explicit session budget at one operation boundary.
    fn budget_exhausted(&self, operation: &str) -> Error {
        Error::InvalidState(format!(
            "run budget exhausted during {operation} ({} ms total)",
            self.run_budget.total_ms()
        ))
    }

    /// Drop driver/socket state after the owned child has been reaped.
    fn finish_cleanup(&mut self) {
        self.driver.reset();
        self.cleaned_up = true;
        remove_control_socket(&self.control_socket);
    }

    /// Forcefully kill the child process and wait for exit.
    pub fn kill_and_wait(&mut self) {
        if self.cleaned_up {
            return;
        }
        if let Err(_e) = self.child.kill_and_wait() {}
        self.finish_cleanup();
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

/// Allocate a unique repository-local control socket path.
fn next_control_socket_path() -> Result<PathBuf> {
    let nonce = NEXT_CONTROL_SOCKET.fetch_add(1, Ordering::Relaxed);
    let directory = env::current_dir()?.join("tmp/app-session");
    fs::create_dir_all(&directory)?;
    Ok(directory.join(format!("hotki-{}-{nonce}.sock", std_process::id())))
}

/// Remove a control socket after graceful or forced cleanup.
fn remove_control_socket(path: &Path) {
    if let Err(error) = fs::remove_file(path)
        && error.kind() != io::ErrorKind::NotFound
    {
        debug!(?error, path = %path.display(), "failed to remove harness control socket");
    }
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
