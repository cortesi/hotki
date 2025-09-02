use std::{env, time::Duration};

use tokio::time::{sleep, timeout};
use tracing::{debug, error, info, warn};

use crate::{
    Error, Result, default_socket_path,
    ipc::Connection,
    process::{ProcessConfig, ServerProcess},
};

// Connection timing constants (internal-only; simplified API)
const STARTUP_POLL_TIMEOUT_MS: u64 = 1000;
const CONNECT_TIMEOUT_SECS: u64 = 5;
const CONNECT_MAX_ATTEMPTS: u32 = 5;
const CONNECT_RETRY_DELAY_MS: u64 = 200;

// Use centralized permissions crate

/// A client for connecting to a hotkey server.
///
/// The client will attempt to connect to an existing server at the configured socket path.
/// If no server is running and auto-spawn is configured, it will spawn a new server process.
///
/// # Server Spawning
///
/// By default, the client will only connect to existing servers. To enable automatic
/// server spawning, use one of these methods:
///
/// - [`with_auto_spawn_server()`](Self::with_auto_spawn_server) - Uses the current executable with `--server` flag
pub struct Client {
    /// Socket path for IPC communication
    socket_path: String,
    /// Optional server configuration (if None, won't spawn server)
    server_config: Option<ProcessConfig>,
    /// The spawned server process (if any)
    server: Option<ServerProcess>,
    /// The active IPC connection (if connected)
    connection: Option<Connection>,
}

impl Default for Client {
    fn default() -> Self {
        Self::new()
    }
}

impl Client {
    /// Create a new managed client with default configuration.
    ///
    /// Defaults to auto-spawning a server (same binary) unless opted out
    /// via [`with_connect_only`]. This matches how the UI uses the client.
    pub fn new() -> Self {
        let base = Self {
            socket_path: default_socket_path().to_string(),
            server_config: None,
            server: None,
            connection: None,
        };
        base.with_auto_spawn_server()
    }

    /// Create a new managed client with the given socket path.
    ///
    /// Like [`new`], this defaults to auto-spawn. Use [`with_connect_only`] to opt out.
    pub fn new_with_socket(socket_path: impl Into<String>) -> Self {
        let base = Self {
            socket_path: socket_path.into(),
            server_config: None,
            server: None,
            connection: None,
        };
        base.with_auto_spawn_server()
    }

    /// Set the socket path
    pub fn with_socket_path(mut self, socket_path: impl Into<String>) -> Self {
        self.socket_path = socket_path.into();
        // Simplified: remove any existing "--socket <...>" pair and append one
        // fresh pair at the end. Preserve all other args as-is.
        if let Some(ref mut config) = self.server_config {
            let mut new_args: Vec<String> = Vec::with_capacity(config.args.len() + 2);
            let mut i = 0;
            while i < config.args.len() {
                if config.args[i] == "--socket" {
                    // Skip option and its value if present
                    i += 1;
                    if i < config.args.len() {
                        i += 1;
                    }
                } else {
                    new_args.push(config.args[i].clone());
                    i += 1;
                }
            }
            new_args.push("--socket".to_string());
            new_args.push(self.socket_path.clone());
            config.args = new_args;
        }
        self
    }

    /// Enable automatic server spawning using the default command.
    ///
    /// The default command is the current executable with the "--server" argument.
    /// This is equivalent to calling `with_server_command(current_exe, ["--server", "--socket", <socket_path>])`.
    pub fn with_auto_spawn_server(mut self) -> Self {
        if let Ok(current_exe) = env::current_exe() {
            let mut config = ProcessConfig::new(current_exe);
            // Pass the socket path to the server so it uses the same one as the client
            config.args = vec![
                "--server".to_string(),
                "--socket".to_string(),
                self.socket_path.clone(),
            ];
            // Propagate our PID so the server can watch the UI and exit
            // immediately if the frontend process goes away for any reason.
            let ppid = std::process::id().to_string();
            // Replace existing HOTKI_PARENT_PID if present, otherwise push
            if let Some((_, v)) = config.env.iter_mut().find(|(k, _)| k == "HOTKI_PARENT_PID") {
                *v = ppid;
            } else {
                config.env.push(("HOTKI_PARENT_PID".to_string(), ppid));
            }
            self.server_config = Some(config);
        }
        self
    }

    /// Propagate a log filter to the spawned server via `RUST_LOG`.
    ///
    /// Call after `with_auto_spawn_server()` (or ensure a server_config exists).
    pub fn with_server_log_filter(mut self, filter: impl Into<String>) -> Self {
        let filter = filter.into();
        // Ensure we have a config to attach env to.
        if self.server_config.is_none()
            && let Ok(current_exe) = env::current_exe()
        {
            let mut config = ProcessConfig::new(current_exe);
            config.args = vec![
                "--server".to_string(),
                "--socket".to_string(),
                self.socket_path.clone(),
            ];
            self.server_config = Some(config);
        }
        if let Some(cfg) = &mut self.server_config {
            // Replace existing RUST_LOG if present, otherwise push
            if let Some((_, v)) = cfg.env.iter_mut().find(|(k, _)| k == "RUST_LOG") {
                *v = filter.clone();
            } else {
                cfg.env.push(("RUST_LOG".to_string(), filter));
            }
        }
        self
    }

    /// Opt-out of auto-spawn behavior and only attempt to connect to an
    /// already-running server.
    pub fn with_connect_only(mut self) -> Self {
        self.server_config = None;
        self
    }

    /// Connect to the server, optionally spawning it first
    pub async fn connect(mut self) -> Result<Self> {
        // Check if we're already connected
        if self.connection.is_some() {
            debug!("Already connected to server");
            return Ok(self);
        }

        // Spawn a managed server if configured
        let mut spawned_server: Option<ServerProcess> = None;
        if let Some(server_config) = &self.server_config {
            info!("Spawning new server at {}", self.socket_path);
            let mut server = ServerProcess::new(server_config.clone());
            server.start().await?;
            spawned_server = Some(server);
        }

        // Unified readiness + retry logic
        match self.try_connect_with_retries().await {
            Ok(conn) => {
                self.connection = Some(conn);
                if let Some(server) = spawned_server {
                    self.server = Some(server);
                }
                Ok(self)
            }
            Err(e) => {
                error!("Failed to connect to server: {}", e);
                let input_ok = permissions::input_monitoring_ok();
                let ax_ok = permissions::accessibility_ok();
                if !input_ok {
                    warn!("Input Monitoring not granted (CGPreflightListenEventAccess=false)");
                }
                if !ax_ok {
                    warn!("Accessibility not granted (AXIsProcessTrusted=false)");
                }
                if let Some(mut server) = spawned_server {
                    // Best effort cleanup
                    let _ = server.stop().await;
                }
                Err(e)
            }
        }
    }

    /// Try to connect to the server once
    async fn try_connect(&self) -> Result<Connection> {
        match timeout(
            Duration::from_secs(CONNECT_TIMEOUT_SECS),
            Connection::connect_unix(&self.socket_path),
        )
        .await
        {
            Ok(Ok(connection)) => Ok(connection),
            Ok(Err(e)) => Err(e),
            Err(_) => Err(Error::Ipc(format!(
                "Connection timeout after {:?}",
                Duration::from_secs(CONNECT_TIMEOUT_SECS)
            ))),
        }
    }

    /// Try to connect with retries; includes a fast startup poll if a managed
    /// server has just been spawned.
    async fn try_connect_with_retries(&self) -> Result<Connection> {
        let mut last_error = None;

        // If we spawned a managed server, do a fast readiness poll window first.
        if self.server.is_some() {
            debug!(
                "Polling for server readiness (timeout: {:?})",
                Duration::from_millis(STARTUP_POLL_TIMEOUT_MS)
            );
            let start_time = tokio::time::Instant::now();
            let mut poll_interval = Duration::from_millis(10);
            while start_time.elapsed() < Duration::from_millis(STARTUP_POLL_TIMEOUT_MS) {
                match self.try_connect().await {
                    Ok(conn) => {
                        info!("Connected to spawned server in {:?}", start_time.elapsed());
                        return Ok(conn);
                    }
                    Err(e) => {
                        last_error = Some(e);
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
                Err(e) => {
                    warn!("Connection attempt {} failed: {}", attempt, e);
                    last_error = Some(e);
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

    /// Get a reference to the connection
    pub fn connection(&mut self) -> Result<&mut Connection> {
        self.connection
            .as_mut()
            .ok_or_else(|| Error::Ipc("Not connected to server".to_string()))
    }

    /// Check if connected
    pub fn is_connected(&self) -> bool {
        self.connection.is_some()
    }

    /// Disconnect from the server and optionally stop it
    pub async fn disconnect(&mut self, stop_server: bool) -> Result<()> {
        // Shutdown the connection
        if let Some(mut connection) = self.connection.take() {
            info!("Shutting down connection");
            connection.shutdown().await?;
        }

        // Stop the server if requested and we spawned it
        if stop_server && let Some(mut server) = self.server.take() {
            info!("Stopping managed server");
            server.stop().await?;
        }

        Ok(())
    }

    /// Gracefully shut down the server via RPC, then stop the managed
    /// process if still running.
    pub async fn shutdown_server(&mut self) -> Result<()> {
        // Request graceful shutdown if connected
        if let Some(conn) = self.connection.as_mut() {
            info!("Requesting server shutdown via RPC");
            conn.shutdown().await?;
        }
        // If we manage a spawned server, ensure the process is stopped
        if let Some(mut server) = self.server.take() {
            info!("Stopping managed server process");
            server.stop().await?;
        }
        Ok(())
    }

    /// Get the PID of the spawned server process, if any.
    ///
    /// Returns `None` if no server was spawned (e.g., connected to an existing server)
    /// or if the server process has terminated.
    pub fn server_pid(&self) -> Option<u32> {
        self.server.as_ref().and_then(|s| s.pid())
    }
}

// preflight helpers are provided by the permissions crate

impl Drop for Client {
    fn drop(&mut self) {
        // Clean disconnect on drop
        if self.is_connected() {
            debug!("ManagedClient dropped while still connected");
            // Can't do async in drop, so connection will close when dropped
        }

        // ServerProcess has its own drop implementation
        if self.server.is_some() {
            debug!("ManagedClient dropped with running server");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_client_builder() {
        let client = Client::new_with_socket("/test/socket.sock");
        assert_eq!(client.socket_path, "/test/socket.sock");
    }

    #[test]
    fn test_client_default_socket_path() {
        let client = Client::new();
        assert_eq!(client.socket_path, default_socket_path());
    }
}
