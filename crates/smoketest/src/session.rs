use std::{
    path::{Path, PathBuf},
    process::{Child, Command},
    time::{Duration, Instant},
};

use crate::{
    config,
    error::{Error, Result},
    runtime,
    ui_interaction::send_activation_chord,
};

/// State tracking for HotkiSession
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionState {
    Starting,
    Running,
    Stopped,
}

/// Builder for HotkiSession configuration
pub struct HotkiSessionBuilder {
    binary_path: PathBuf,
    config_path: Option<PathBuf>,
    with_logs: bool,
}

impl HotkiSessionBuilder {
    pub fn new(binary_path: impl Into<PathBuf>) -> Self {
        Self {
            binary_path: binary_path.into(),
            config_path: None,
            with_logs: false,
        }
    }

    pub fn with_config(mut self, path: impl Into<PathBuf>) -> Self {
        self.config_path = Some(path.into());
        self
    }

    pub fn with_logs(mut self, enable: bool) -> Self {
        self.with_logs = enable;
        self
    }

    pub fn spawn(self) -> Result<HotkiSession> {
        let mut cmd = Command::new(&self.binary_path);

        if self.with_logs {
            cmd.env("RUST_LOG", config::TEST_LOG_CONFIG);
        }

        if let Some(cfg) = &self.config_path {
            cmd.arg(cfg);
        }

        let child = cmd.spawn().map_err(|e| Error::SpawnFailed(e.to_string()))?;

        let socket_path = socket_path_for_pid(child.id());

        Ok(HotkiSession {
            child,
            socket_path,
            state: SessionState::Starting,
        })
    }
}

pub struct HotkiSession {
    child: Child,
    socket_path: String,
    state: SessionState,
}

impl HotkiSession {
    /// Create a new session builder
    pub fn builder(binary_path: impl Into<PathBuf>) -> HotkiSessionBuilder {
        HotkiSessionBuilder::new(binary_path)
    }

    /// Legacy constructor for compatibility
    pub fn launch_with_config(
        hotki_bin: &Path,
        cfg_path: &Path,
        with_logs: bool,
    ) -> Result<HotkiSession> {
        Self::builder(hotki_bin)
            .with_config(cfg_path)
            .with_logs(with_logs)
            .spawn()
    }

    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    pub fn socket_path(&self) -> &str {
        &self.socket_path
    }

    pub fn wait_for_hud(&mut self, timeout_ms: u64) -> (bool, u64) {
        // Try to connect and wait for HudUpdate indicating HUD visible.
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
                        return (false, start.elapsed().as_millis() as u64);
                    }
                    let delay = if attempts <= config::INITIAL_RETRY_ATTEMPTS {
                        config::INITIAL_RETRY_DELAY_MS
                    } else {
                        config::FAST_RETRY_DELAY_MS
                    };
                    std::thread::sleep(Duration::from_millis(delay));
                    continue;
                }
            }
        };

        // Mark as running once connected
        self.state = SessionState::Running;

        // Borrow connection
        let conn = match client.connection() {
            Ok(c) => c,
            Err(_) => return (false, start.elapsed().as_millis() as u64),
        };

        // Send activation chord periodically until HUD visible
        send_activation_chord();
        let mut last_sent = Some(Instant::now());

        while Instant::now() < deadline {
            let left = deadline.saturating_duration_since(Instant::now());
            let chunk = std::cmp::min(left, config::ms(config::EVENT_CHECK_INTERVAL_MS));
            let res = runtime::block_on(async { tokio::time::timeout(chunk, conn.recv_event()).await });
            match res {
                Ok(Ok(Ok(msg))) => {
                    if let hotki_protocol::MsgToUI::HudUpdate { cursor, .. } = msg {
                        let depth = cursor.depth();
                        let visible = cursor.viewing_root || depth > 0;
                        if visible {
                            return (true, start.elapsed().as_millis() as u64);
                        }
                    }
                }
                Ok(Ok(Err(_))) => break,
                Ok(Err(_)) | Err(_) => {}
            }
            // Smart side-check: look for the HUD window by title under the hotki server pid
            // to avoid missing HudUpdate races.
            if mac_winops::list_windows()
                .into_iter()
                .any(|w| w.pid == self.pid() as i32 && w.title == "Hotki HUD")
            {
                return (true, start.elapsed().as_millis() as u64);
            }
            if let Some(last) = last_sent
                && last.elapsed() >= Duration::from_millis(1000)
            {
                send_activation_chord();
                last_sent = Some(Instant::now());
            }
        }
        (false, start.elapsed().as_millis() as u64)
    }

    pub fn shutdown(&mut self) {
        let sock = self.socket_path.clone();
        let _ = runtime::block_on(async move {
            if let Ok(mut c) = hotki_server::Client::new_with_socket(&sock)
                .with_connect_only()
                .connect()
                .await
            {
                let _ = c.shutdown_server().await;
            }
        });
        self.state = SessionState::Stopped;
    }

    pub fn kill_and_wait(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
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
