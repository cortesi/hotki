use tracing::{debug, info};

use crate::{Error, Result, ipc::Connection, managed_server::ManagedServer};

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
    /// Managed server launch policy and spawned process state.
    managed_server: ManagedServer,
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
            managed_server: ManagedServer::new(),
            connection: None,
        };
        base.with_auto_spawn_server()
    }

    /// Create a new managed client with the given socket path.
    ///
    /// Like [`new`], this defaults to auto-spawn. Use [`with_connect_only`] to opt out.
    pub fn new_with_socket(socket_path: impl Into<String>) -> Self {
        let base = Self {
            managed_server: ManagedServer::new_with_socket(socket_path),
            connection: None,
        };
        base.with_auto_spawn_server()
    }

    /// Set the socket path
    pub fn with_socket_path(mut self, socket_path: impl Into<String>) -> Self {
        self.managed_server.set_socket_path(socket_path);
        self
    }

    /// Enable automatic server spawning using the default command.
    ///
    /// The default command is the current executable with `--server` and a
    /// `--socket <path>` pair matching this client's `socket_path`.
    ///
    /// Idempotent: calling this multiple times preserves existing `env` and
    /// other args, ensures a single `--server`, and replaces any prior
    /// `--socket` pair with the current `socket_path`.
    pub fn with_auto_spawn_server(mut self) -> Self {
        self.managed_server.enable_auto_spawn_server();
        self
    }

    /// Propagate a log filter to the spawned server via `RUST_LOG`.
    ///
    /// Order independent: may be called before or after
    /// [`with_auto_spawn_server`]. If no server config exists yet, this method
    /// seeds one using the current executable and the correct `--server` and
    /// `--socket` args.
    pub fn with_server_log_filter(mut self, filter: impl Into<String>) -> Self {
        self.managed_server.set_server_log_filter(filter);
        self
    }

    /// Opt-out of auto-spawn behavior and only attempt to connect to an
    /// already-running server.
    pub fn with_connect_only(mut self) -> Self {
        self.managed_server.disable_auto_spawn();
        self
    }

    /// Connect to the server, optionally spawning it first.
    ///
    /// If configured for auto‑spawn, this launches the server and performs a
    /// short, fast readiness poll before falling back to bounded retries. On
    /// success, the returned `Client` holds an active `Connection`.
    pub async fn connect(mut self) -> Result<Self> {
        if self.connection.is_some() {
            debug!("Already connected to server");
            return Ok(self);
        }

        self.connection = Some(self.managed_server.connect().await?);
        Ok(self)
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
        if stop_server {
            self.managed_server.stop_server().await?;
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
        self.managed_server.stop_server().await?;
        Ok(())
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
        if self.managed_server.has_server() {
            debug!("ManagedClient dropped with running server");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn count_flag(args: &[String], flag: &str) -> usize {
        args.iter().filter(|a| a.as_str() == flag).count()
    }

    #[test]
    fn client_default_socket_path() {
        let client = Client::new();
        assert_eq!(
            client.managed_server.socket_path(),
            crate::default_socket_path()
        );
        // auto-spawn seeded
        let cfg = client
            .managed_server
            .server_config()
            .expect("server config");
        assert!(cfg.args.iter().any(|a| a == "--server"));
        // expect a single --socket pair
        assert_eq!(count_flag(&cfg.args, "--socket"), 1);
    }

    #[test]
    fn with_socket_path_updates_args_after_auto() {
        let client = Client::new().with_socket_path("/tmp/custom.sock");
        let cfg = client
            .managed_server
            .server_config()
            .expect("server config");
        // exactly one socket flag and value equals the client's socket_path
        assert_eq!(count_flag(&cfg.args, "--socket"), 1);
        let idx = cfg.args.iter().position(|a| a == "--socket").unwrap();
        assert_eq!(cfg.args[idx + 1], "/tmp/custom.sock");
    }

    #[test]
    fn with_socket_path_before_auto_then_auto() {
        // Start connect-only (no server_config), change path, then enable auto
        let client = Client::new()
            .with_connect_only()
            .with_socket_path("/tmp/early.sock")
            .with_auto_spawn_server();
        let cfg = client
            .managed_server
            .server_config()
            .expect("server config");
        assert_eq!(count_flag(&cfg.args, "--socket"), 1);
        let idx = cfg.args.iter().position(|a| a == "--socket").unwrap();
        assert_eq!(cfg.args[idx + 1], "/tmp/early.sock");
        assert!(cfg.args.iter().any(|a| a == "--server"));
    }

    #[test]
    fn auto_spawn_idempotent_no_dup_flags() {
        let client = Client::new()
            .with_auto_spawn_server() // already auto; call again to test idempotency
            .with_auto_spawn_server();
        let cfg = client
            .managed_server
            .server_config()
            .expect("server config");
        // Only one --server and one --socket
        assert_eq!(count_flag(&cfg.args, "--server"), 1);
        assert_eq!(count_flag(&cfg.args, "--socket"), 1);
        // HOTKI_PARENT_PID is present
        assert!(cfg.env.iter().any(|(k, _)| k == "HOTKI_PARENT_PID"));
    }

    #[test]
    fn log_filter_order_independent() {
        // filter before auto
        let c1 = Client::new()
            .with_connect_only()
            .with_server_log_filter("a=info")
            .with_auto_spawn_server();
        let cfg1 = c1.managed_server.server_config().expect("server config");
        assert_eq!(
            cfg1.env
                .iter()
                .find(|(k, _)| k == "RUST_LOG")
                .map(|(_, v)| v.as_str()),
            Some("a=info")
        );

        // filter after auto
        let c2 = Client::new()
            .with_auto_spawn_server()
            .with_server_log_filter("b=debug");
        let cfg2 = c2.managed_server.server_config().expect("server config");
        assert_eq!(
            cfg2.env
                .iter()
                .find(|(k, _)| k == "RUST_LOG")
                .map(|(_, v)| v.as_str()),
            Some("b=debug")
        );

        // filter override
        let c3 = Client::new()
            .with_auto_spawn_server()
            .with_server_log_filter("first")
            .with_server_log_filter("second");
        let cfg3 = c3.managed_server.server_config().expect("server config");
        assert_eq!(
            cfg3.env
                .iter()
                .find(|(k, _)| k == "RUST_LOG")
                .map(|(_, v)| v.as_str()),
            Some("second")
        );
    }
}
