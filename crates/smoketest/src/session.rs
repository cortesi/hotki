use std::{
    env, fs,
    io::ErrorKind,
    path::PathBuf,
    process::{self as std_process, Command},
    time::{SystemTime, UNIX_EPOCH},
};

use logging as logshared;
use tracing::debug;

use crate::{
    config,
    error::{Error, Result},
    process::{self, ManagedChild},
    server_drive::{BridgeDriver, ControlSocketScope},
};

/// Launch configuration for a smoketest-backed hotki session.
pub struct HotkiSessionConfig {
    /// Path to the hotki binary to run.
    binary_path: PathBuf,
    /// Optional path to a config file to load.
    config_path: Option<PathBuf>,
    /// Whether to enable verbose logs for the child.
    with_logs: bool,
}

impl HotkiSessionConfig {
    /// Construct a configuration using the default hotki binary resolution.
    pub fn from_env() -> Result<Self> {
        Ok(Self {
            binary_path: resolve_hotki_binary()?,
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
    /// Path to the control bridge socket exposed by the UI runtime.
    control_socket: String,
    /// Bridge control socket override guard for this session.
    _control_scope: ControlSocketScope,
    /// Driver handle used to communicate with the smoketest bridge.
    bridge: BridgeDriver,
    /// Whether teardown has already been performed.
    cleaned_up: bool,
}

/// Ensures the control socket is torn down if session bootstrap fails mid-way.
struct ControlSocketCleanup {
    /// Filesystem path to the bridge control socket for this bootstrap attempt.
    path: String,
    /// Whether cleanup should run when the guard is dropped.
    active: bool,
}

impl ControlSocketCleanup {
    /// Create a guard that will remove the control socket path if not disarmed.
    fn new(path: String) -> Self {
        Self { path, active: true }
    }

    /// Prevent the guard from removing the socket path on drop.
    fn disarm(&mut self) {
        self.active = false;
    }
}

impl Drop for ControlSocketCleanup {
    fn drop(&mut self) {
        if !self.active {
            return;
        }
        if let Err(err) = fs::remove_file(&self.path)
            && err.kind() != ErrorKind::NotFound
        {
            debug!(
                ?err,
                socket = %self.path,
                "failed to remove control socket during bootstrap cleanup"
            );
        }
        self.active = false;
    }
}

impl HotkiSession {
    /// Spawn a hotki process according to the supplied configuration.
    pub fn spawn(config: HotkiSessionConfig) -> Result<Self> {
        let HotkiSessionConfig {
            binary_path,
            config_path,
            with_logs,
        } = config;
        let control_socket = unique_control_socket_path();
        let mut cleanup_guard = ControlSocketCleanup::new(control_socket.clone());
        let control_scope = ControlSocketScope::new(control_socket.clone());
        if let Err(err) = fs::remove_file(&control_socket)
            && err.kind() != ErrorKind::NotFound
        {
            debug!(
                ?err,
                socket = %control_socket,
                "failed to remove stale control socket"
            );
        }

        let mut cmd = Command::new(&binary_path);
        if with_logs {
            cmd.env("RUST_LOG", logshared::log_config_for_child());
        }
        if let Some(cfg) = &config_path {
            cmd.arg("--config");
            cmd.arg(cfg);
        }
        cmd.env("HOTKI_CONTROL_SOCKET", &control_socket);

        let mut child = process::spawn_managed(cmd)?;
        let mut bridge = BridgeDriver::new(socket_path_for_pid(child.pid as u32));
        if let Err(err) = bridge.ensure_ready(config::DEFAULTS.timeout_ms) {
            if let Err(kill_err) = child.kill_and_wait() {
                debug!(
                    ?kill_err,
                    "failed to terminate hotki after bridge init failure"
                );
            }
            return Err(Error::from(err));
        }
        cleanup_guard.disarm();
        Ok(Self {
            child,
            control_socket,
            _control_scope: control_scope,
            bridge,
            cleaned_up: false,
        })
    }

    /// Return the OS process id for the hotki child.
    pub fn pid(&self) -> u32 {
        self.child.pid as u32
    }

    /// Borrow the bridge driver mutably.
    pub fn bridge_mut(&mut self) -> &mut BridgeDriver {
        &mut self.bridge
    }

    /// Attempt a graceful server shutdown via the bridge, surfacing failures.
    pub fn shutdown(&mut self) -> Result<()> {
        if self.cleaned_up {
            return Ok(());
        }
        self.bridge.shutdown().map_err(Error::from)
    }

    /// Forcefully kill the child process and wait for exit.
    pub fn kill_and_wait(&mut self) {
        if self.cleaned_up {
            return;
        }
        if let Err(_e) = self.child.kill_and_wait() {}
        self.bridge.reset();
        if let Err(err) = fs::remove_file(&self.control_socket)
            && err.kind() != ErrorKind::NotFound
        {
            tracing::debug!(?err, socket = %self.control_socket, "failed to remove control socket");
        }
        self.cleaned_up = true;
    }
}

impl Drop for HotkiSession {
    fn drop(&mut self) {
        if self.cleaned_up {
            return;
        }
        if let Err(err) = self.shutdown() {
            debug!(?err, "bridge shutdown during drop failed");
        }
        self.kill_and_wait();
    }
}

// ===== Socket Path Management =====

/// Generate the socket path for a given process ID
pub fn socket_path_for_pid(pid: u32) -> String {
    hotki_server::socket_path_for_pid(pid)
}

/// Generate a unique control socket path under the system temporary directory.
fn unique_control_socket_path() -> String {
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let pid = std_process::id();
    env::temp_dir()
        .join(format!("hotki-bridge-{pid}-{ts}.sock"))
        .to_string_lossy()
        .into_owned()
}

/// Resolve the hotki binary path from env overrides or the current executable dir.
fn resolve_hotki_binary() -> Result<PathBuf> {
    if let Ok(path) = env::var("HOTKI_BIN") {
        let candidate = PathBuf::from(path);
        if candidate.exists() {
            return Ok(candidate);
        }
    }

    let inferred = env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join("hotki")))
        .filter(|path| path.exists());

    inferred.ok_or(Error::HotkiBinNotFound)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn spawn_initializes_bridge() -> Result<()> {
        let config = match HotkiSessionConfig::from_env() {
            Ok(cfg) => cfg.with_logs(false),
            Err(Error::HotkiBinNotFound) => return Ok(()),
            Err(other) => return Err(other),
        };
        let mut session = HotkiSession::spawn(config)?;

        session.bridge_mut().check_alive()?;

        session.shutdown()?;
        session.kill_and_wait();
        Ok(())
    }
}
