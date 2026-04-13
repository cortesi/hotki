use std::{env, time::Duration};

use tokio::time::{sleep, timeout};
use tracing::{debug, error, info, warn};

use crate::{
    Error, Result, default_socket_path,
    ipc::Connection,
    process::{ProcessConfig, ServerProcess},
};

const STARTUP_POLL_TIMEOUT_MS: u64 = 1000;
const CONNECT_TIMEOUT_SECS: u64 = 5;
const CONNECT_MAX_ATTEMPTS: u32 = 5;
const CONNECT_RETRY_DELAY_MS: u64 = 200;

/// Auto-spawn policy and managed server process lifecycle for a client.
pub(crate) struct ManagedServer {
    socket_path: String,
    config: Option<ProcessConfig>,
    server: Option<ServerProcess>,
}

impl ManagedServer {
    /// Build a managed server policy using the default per-process socket path.
    pub(crate) fn new() -> Self {
        Self::new_with_socket(default_socket_path())
    }

    /// Build a managed server policy for a specific socket path.
    pub(crate) fn new_with_socket(socket_path: impl Into<String>) -> Self {
        Self {
            socket_path: socket_path.into(),
            config: None,
            server: None,
        }
    }

    /// Return the active socket path.
    #[cfg(test)]
    pub(crate) fn socket_path(&self) -> &str {
        &self.socket_path
    }

    /// Return the current managed process config, if auto-spawn is enabled.
    #[cfg(test)]
    pub(crate) fn server_config(&self) -> Option<&ProcessConfig> {
        self.config.as_ref()
    }

    /// Return true when a managed process is still tracked.
    pub(crate) fn has_server(&self) -> bool {
        self.server.is_some()
    }

    /// Update the socket path and keep any managed process config in sync.
    pub(crate) fn set_socket_path(&mut self, socket_path: impl Into<String>) {
        self.socket_path = socket_path.into();
        if let Some(config) = &mut self.config {
            config.set_socket_path(&self.socket_path);
        }
    }

    /// Enable automatic server spawning using the current executable.
    pub(crate) fn enable_auto_spawn_server(&mut self) {
        if let Ok(current_exe) = env::current_exe() {
            let mut config = self
                .config
                .take()
                .unwrap_or_else(|| ProcessConfig::new(current_exe.clone()));
            config.executable = current_exe;
            config.ensure_server_mode();
            config.set_socket_path(&self.socket_path);
            config.set_parent_pid(std::process::id());
            self.config = Some(config);
        }
    }

    /// Propagate a log filter to any auto-spawned server via `--log-filter`.
    pub(crate) fn set_server_log_filter(&mut self, filter: impl Into<String>) {
        if self.config.is_none()
            && let Ok(current_exe) = env::current_exe()
        {
            let mut config = ProcessConfig::new(current_exe);
            config.set_socket_path(&self.socket_path);
            self.config = Some(config);
        }
        if let Some(config) = &mut self.config {
            config.set_log_filter(&filter.into());
        }
    }

    /// Disable automatic server spawning and only connect to existing servers.
    pub(crate) fn disable_auto_spawn(&mut self) {
        self.config = None;
    }

    /// Connect to the server, spawning and tracking a managed process if configured.
    pub(crate) async fn connect(&mut self) -> Result<Connection> {
        let mut spawned_server = None;
        if let Some(config) = &self.config {
            debug!("Spawning new server at {}", self.socket_path);
            let mut server = ServerProcess::new(config.clone());
            server.start().await?;
            spawned_server = Some(server);
        }

        let spawned = spawned_server.is_some();
        match self.try_connect_with_retries(spawned).await {
            Ok(connection) => {
                self.server = spawned_server;
                Ok(connection)
            }
            Err(err) => {
                error!("Failed to connect to server: {}", err);
                let input_ok = permissions::input_monitoring_ok();
                let ax_ok = permissions::accessibility_ok();
                if !input_ok {
                    warn!("Input Monitoring not granted (CGPreflightListenEventAccess=false)");
                }
                if !ax_ok {
                    warn!("Accessibility not granted (AXIsProcessTrusted=false)");
                }
                if let Some(mut server) = spawned_server {
                    let _ = server.stop().await;
                }
                Err(err)
            }
        }
    }

    /// Stop the managed server process, if one is currently running.
    pub(crate) async fn stop_server(&mut self) -> Result<()> {
        if let Some(mut server) = self.server.take() {
            info!("Stopping managed server");
            server.stop().await?;
        }
        Ok(())
    }

    async fn try_connect(&self) -> Result<Connection> {
        match timeout(
            Duration::from_secs(CONNECT_TIMEOUT_SECS),
            Connection::connect_unix(&self.socket_path),
        )
        .await
        {
            Ok(Ok(connection)) => Ok(connection),
            Ok(Err(err)) => Err(err),
            Err(_) => Err(Error::Ipc(format!(
                "Connection timeout after {:?}",
                Duration::from_secs(CONNECT_TIMEOUT_SECS)
            ))),
        }
    }

    async fn try_connect_with_retries(&self, spawned: bool) -> Result<Connection> {
        let mut last_error = None;

        if spawned {
            debug!(
                "Polling for server readiness (timeout: {:?})",
                Duration::from_millis(STARTUP_POLL_TIMEOUT_MS)
            );
            let start_time = tokio::time::Instant::now();
            let mut poll_interval = Duration::from_millis(10);
            while start_time.elapsed() < Duration::from_millis(STARTUP_POLL_TIMEOUT_MS) {
                match self.try_connect().await {
                    Ok(connection) => {
                        info!("Connected to spawned server in {:?}", start_time.elapsed());
                        return Ok(connection);
                    }
                    Err(err) => {
                        last_error = Some(err);
                        sleep(poll_interval).await;
                        if poll_interval < Duration::from_millis(100) {
                            poll_interval = poll_interval.saturating_add(Duration::from_millis(10));
                        }
                    }
                }
            }
            debug!("Startup poll window elapsed; falling back to standard retries");
        }

        for attempt in 1..=CONNECT_MAX_ATTEMPTS {
            debug!("Connection attempt {}/{}", attempt, CONNECT_MAX_ATTEMPTS);
            match self.try_connect().await {
                Ok(connection) => return Ok(connection),
                Err(err) => {
                    debug!("Connection attempt {} failed: {}", attempt, err);
                    last_error = Some(err);
                    if attempt < CONNECT_MAX_ATTEMPTS {
                        sleep(Duration::from_millis(CONNECT_RETRY_DELAY_MS)).await;
                    }
                }
            }
        }

        Err(last_error.unwrap_or_else(|| {
            Error::Ipc("Failed to connect after all retry attempts".to_string())
        }))
    }
}
