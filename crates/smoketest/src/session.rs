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
    server_drive::{self, DriverError},
    world,
};

/// Launch configuration for a smoketest-backed hotki session.
pub struct HotkiSessionConfig {
    /// Path to the hotki binary to run.
    binary_path: PathBuf,
    /// Optional path to a config RON file to load.
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
    /// Path to the server's unix socket for the session.
    socket_path: String,
    /// Path to the control bridge socket exposed by the UI runtime.
    control_socket: String,
    /// Whether teardown has already been performed.
    cleaned_up: bool,
}

impl HotkiSession {
    /// Spawn a hotki process according to the supplied configuration.
    pub fn spawn(config: HotkiSessionConfig) -> Result<Self> {
        let HotkiSessionConfig {
            binary_path,
            config_path,
            with_logs,
        } = config;
        let mut cmd = Command::new(&binary_path);
        if with_logs {
            cmd.env("RUST_LOG", logshared::log_config_for_child());
        }
        if let Some(cfg) = &config_path {
            cmd.arg(cfg);
        }
        let control_socket = unique_control_socket_path();
        unsafe {
            env::set_var("HOTKI_CONTROL_SOCKET", &control_socket);
        }
        cmd.env("HOTKI_CONTROL_SOCKET", &control_socket);
        if let Err(err) = fs::remove_file(&control_socket)
            && err.kind() != ErrorKind::NotFound
        {
            tracing::debug!(?err, socket = %control_socket, "failed to remove stale control socket");
        }

        let mut child = process::spawn_managed(cmd)?;
        let socket_path = socket_path_for_pid(child.pid as u32);
        server_drive::reset();
        if let Err(err) = server_drive::ensure_init(&socket_path, config::DEFAULTS.timeout_ms) {
            if let Err(kill_err) = child.kill_and_wait() {
                debug!(
                    ?kill_err,
                    "failed to terminate hotki after bridge init failure"
                );
            }
            return Err(Error::from(err));
        }
        Ok(Self {
            child,
            socket_path,
            control_socket,
            cleaned_up: false,
        })
    }

    /// Return the OS process id for the hotki child.
    pub fn pid(&self) -> u32 {
        self.child.pid as u32
    }

    /// Return the server socket path for the session.
    pub fn socket_path(&self) -> &str {
        &self.socket_path
    }

    /// Attempt a graceful server shutdown via RPC (best-effort).
    pub fn shutdown(&self) {
        if self.cleaned_up {
            return;
        }
        match server_drive::shutdown() {
            Ok(()) => return,
            Err(DriverError::NotInitialized) => {}
            Err(err) => {
                debug!(
                    ?err,
                    "shared MRPC shutdown failed; retrying with direct client"
                );
            }
        }

        let sock = self.socket_path.clone();
        drop(world::block_on(async move {
            if let Ok(mut c) = hotki_server::Client::new_with_socket(&sock)
                .with_connect_only()
                .connect()
                .await
            {
                drop(c.shutdown_server().await);
            }
            Ok::<(), Error>(())
        }));
    }

    /// Forcefully kill the child process and wait for exit.
    pub fn kill_and_wait(&mut self) {
        if self.cleaned_up {
            return;
        }
        if let Err(_e) = self.child.kill_and_wait() {}
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
        self.shutdown();
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
    use crate::server_drive;

    #[test]
    fn spawn_initializes_bridge() -> Result<()> {
        server_drive::reset();
        let config = match HotkiSessionConfig::from_env() {
            Ok(cfg) => cfg.with_logs(false),
            Err(Error::HotkiBinNotFound) => return Ok(()),
            Err(other) => return Err(other),
        };
        let mut session = HotkiSession::spawn(config)?;

        assert!(
            server_drive::is_ready(),
            "bridge should be ready immediately after spawning"
        );
        server_drive::check_alive()?;

        session.shutdown();
        session.kill_and_wait();
        server_drive::reset();
        Ok(())
    }
}
