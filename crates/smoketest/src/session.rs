use std::{
    cmp,
    path::{Path, PathBuf},
    process::{Child, Command},
    thread,
    time::{Duration, Instant},
};

use logging as logshared;
use tokio::time::timeout;

use crate::{
    config,
    error::{Error, Result},
    proc_registry, runtime, server_drive,
    ui_interaction::send_activation_chord,
    world,
};

/// State tracking for HotkiSession
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionState {
    /// Process launched; waiting for readiness
    Starting,
    /// Live connection established
    Running,
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

    /// Legacy constructor for compatibility
    /// Convenience constructor that builds and launches in one call.
    pub fn launch_with_config(hotki_bin: &Path, cfg_path: &Path, with_logs: bool) -> Result<Self> {
        Self::builder(hotki_bin)
            .with_config(cfg_path)
            .with_logs(with_logs)
            .spawn()
    }

    /// Return the OS process id for the hotki child.
    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    /// Return the server socket path for the session.
    pub fn socket_path(&self) -> &str {
        &self.socket_path
    }

    /// Preferred HUD wait with explicit IPC disconnect detection.
    ///
    /// - Ok(elapsed_ms) when HUD becomes visible
    /// - Err(IpcDisconnected) if the MRPC event channel closes unexpectedly
    /// - Err(HudNotVisible) if the timeout elapses without visibility
    pub fn wait_for_hud_checked(&mut self, timeout_ms: u64) -> Result<u64> {
        let deadline = Instant::now() + Duration::from_millis(timeout_ms);
        let start = Instant::now();

        // Connect with retry
        let mut attempts = 0;
        let mut client = loop {
            match runtime::block_on(async {
                hotki_server::Client::new_with_socket(self.socket_path())
                    .with_connect_only()
                    .connect()
                    .await
            }) {
                Ok(Ok(c)) => break c,
                Ok(Err(_)) | Err(_) => {
                    attempts += 1;
                    if Instant::now() >= deadline {
                        return Err(Error::HudNotVisible { timeout_ms });
                    }
                    let delay = if attempts <= config::RETRY.initial_attempts {
                        config::RETRY.initial_delay_ms
                    } else {
                        config::RETRY.fast_delay_ms
                    };
                    thread::sleep(Duration::from_millis(delay));
                    continue;
                }
            }
        };

        // Mark as running once connected
        self.state = SessionState::Running;

        if !server_drive::is_ready() {
            server_drive::init(self.socket_path())?;
        }

        // Borrow connection
        let conn = match client.connection() {
            Ok(c) => c,
            Err(_) => {
                return Err(Error::IpcDisconnected {
                    during: "waiting for HUD",
                });
            }
        };

        // Send activation chord periodically until HUD visible
        send_activation_chord()?;
        let mut last_sent = Some(Instant::now());

        while Instant::now() < deadline {
            let left = deadline.saturating_duration_since(Instant::now());
            let chunk = cmp::min(left, config::ms(config::RETRY.event_check_interval_ms));
            let res = runtime::block_on(async { timeout(chunk, conn.recv_event()).await });
            match res {
                Ok(Ok(Ok(msg))) => {
                    if let hotki_protocol::MsgToUI::HudUpdate { cursor, .. } = msg {
                        let depth = cursor.depth();
                        let visible = cursor.viewing_root || depth > 0;
                        if visible {
                            return Ok(start.elapsed().as_millis() as u64);
                        }
                    }
                }
                Ok(Ok(Err(_))) => {
                    return Err(Error::IpcDisconnected {
                        during: "waiting for HUD",
                    });
                }
                Ok(Err(_)) | Err(_) => {}
            }
            // Side-check to catch HUD presence even if we missed an event.
            if world::list_windows_or_empty()
                .into_iter()
                .any(|w| w.pid == self.pid() as i32 && w.title == "Hotki HUD")
            {
                return Ok(start.elapsed().as_millis() as u64);
            }
            if let Some(last) = last_sent
                && last.elapsed()
                    >= Duration::from_millis(config::SESSION.activation_resend_interval_ms)
            {
                send_activation_chord()?;
                last_sent = Some(Instant::now());
            }
        }
        Err(Error::HudNotVisible { timeout_ms })
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
