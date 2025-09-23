use std::{
    path::PathBuf,
    process::{Child, Command},
};

use logging as logshared;

use crate::{
    error::{Error, Result},
    proc_registry, runtime,
};

/// State tracking for HotkiSession
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionState {
    /// Process launched; waiting for readiness
    Starting,
    /// Session stopped or cleaned up
    Stopped,
}

/// Builder for HotkiSession configuration
pub struct HotkiSessionBuilder {
    /// Path to the hotki binary to run.
    binary_path: PathBuf,
    /// Optional path to a config RON file to load.
    config_path: Option<PathBuf>,
    /// Whether to enable verbose logs for the child.
    with_logs: bool,
}

impl HotkiSessionBuilder {
    /// Create a new session builder for the given binary path.
    pub fn new(binary_path: impl Into<PathBuf>) -> Self {
        Self {
            binary_path: binary_path.into(),
            config_path: None,
            with_logs: false,
        }
    }

    /// Provide a configuration file path to the hotki process.
    pub fn with_config(mut self, path: impl Into<PathBuf>) -> Self {
        self.config_path = Some(path.into());
        self
    }

    /// Enable or disable child process logging via `RUST_LOG`.
    pub fn with_logs(mut self, enable: bool) -> Self {
        self.with_logs = enable;
        self
    }

    /// Spawn the hotki process and return a running session.
    pub fn spawn(self) -> Result<HotkiSession> {
        let mut cmd = Command::new(&self.binary_path);

        if self.with_logs {
            cmd.env("RUST_LOG", logshared::log_config_for_child());
        }

        if let Some(cfg) = &self.config_path {
            cmd.arg(cfg);
        }

        let child = cmd.spawn().map_err(|e| Error::SpawnFailed(e.to_string()))?;

        let socket_path = socket_path_for_pid(child.id());
        proc_registry::register(child.id() as i32);

        Ok(HotkiSession {
            child,
            socket_path,
            state: SessionState::Starting,
        })
    }
}

/// Running hotki process with helpers for RPC and shutdown.
pub struct HotkiSession {
    /// Child process handle.
    child: Child,
    /// Path to the server's unix socket for this process.
    socket_path: String,
    /// Current session state.
    state: SessionState,
}

impl HotkiSession {
    /// Create a new session builder
    /// Create a new session builder.
    pub fn builder(binary_path: impl Into<PathBuf>) -> HotkiSessionBuilder {
        HotkiSessionBuilder::new(binary_path)
    }

    /// Return the OS process id for the hotki child.
    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    /// Return the server socket path for the session.
    pub fn socket_path(&self) -> &str {
        &self.socket_path
    }

    /// Attempt a graceful server shutdown via RPC (best-effort).
    pub fn shutdown(&mut self) {
        let sock = self.socket_path.clone();
        drop(runtime::block_on(async move {
            if let Ok(mut c) = hotki_server::Client::new_with_socket(&sock)
                .with_connect_only()
                .connect()
                .await
            {
                drop(c.shutdown_server().await);
            }
            Ok::<(), Error>(())
        }));
        self.state = SessionState::Stopped;
    }

    /// Forcefully kill the child process and wait for exit.
    pub fn kill_and_wait(&mut self) {
        let pid = self.child.id() as i32;
        if let Err(_e) = self.child.kill() {}
        if let Err(_e) = self.child.wait() {}
        proc_registry::unregister(pid);
        self.state = SessionState::Stopped;
    }
}

impl Drop for HotkiSession {
    fn drop(&mut self) {
        if self.state != SessionState::Stopped {
            self.shutdown();
            self.kill_and_wait();
        }
    }
}

// ===== Socket Path Management =====

/// Generate the socket path for a given process ID
pub fn socket_path_for_pid(pid: u32) -> String {
    hotki_server::socket_path_for_pid(pid)
}
