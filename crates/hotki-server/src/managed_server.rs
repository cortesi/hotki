use std::{
    env,
    process::{self, ExitStatus},
    time::Duration,
};

use tokio::time::{sleep, timeout};
use tracing::{debug, error, info, warn};

use crate::{
    Error, Result, default_socket_path,
    ipc::Connection,
    process::{ProcessConfig, ServerProcess},
};

const DEFAULT_RETRY_POLICY: RetryPolicy = RetryPolicy {
    startup_poll_timeout: Duration::from_millis(1000),
    startup_initial_delay: Duration::from_millis(10),
    startup_max_delay: Duration::from_millis(100),
    startup_delay_step: Duration::from_millis(10),
    connect_timeout: Duration::from_secs(5),
    connect_attempts: 5,
    connect_retry_delay: Duration::from_millis(200),
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RetryPolicy {
    startup_poll_timeout: Duration,
    startup_initial_delay: Duration,
    startup_max_delay: Duration,
    startup_delay_step: Duration,
    connect_timeout: Duration,
    connect_attempts: u32,
    connect_retry_delay: Duration,
}

impl RetryPolicy {
    fn next_startup_delay(self, current: Duration) -> Duration {
        current
            .saturating_add(self.startup_delay_step)
            .min(self.startup_max_delay)
    }
}

impl Default for RetryPolicy {
    fn default() -> Self {
        DEFAULT_RETRY_POLICY
    }
}

/// Auto-spawn policy and managed server process lifecycle for a client.
pub(crate) struct ManagedServer {
    socket_path: String,
    config: Option<ProcessConfig>,
    server: Option<ServerProcess>,
    retry_policy: RetryPolicy,
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
            retry_policy: RetryPolicy::default(),
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

    #[cfg(test)]
    fn retry_policy(&self) -> RetryPolicy {
        self.retry_policy
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
            config.set_executable(current_exe);
            config.ensure_server_mode();
            config.set_socket_path(&self.socket_path);
            config.set_parent_pid(process::id());
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

    /// Configure whether an auto-spawned server starts the physical keyboard event tap.
    pub(crate) fn set_server_event_tap_enabled(&mut self, enabled: bool) {
        if self.config.is_none()
            && let Ok(current_exe) = env::current_exe()
        {
            let mut config = ProcessConfig::new(current_exe);
            config.set_socket_path(&self.socket_path);
            self.config = Some(config);
        }
        if let Some(config) = &mut self.config {
            config.set_event_tap_enabled(enabled);
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

        match self.try_connect_with_retries(spawned_server.as_mut()).await {
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
            self.retry_policy.connect_timeout,
            Connection::connect_unix(&self.socket_path),
        )
        .await
        {
            Ok(Ok(connection)) => Ok(connection),
            Ok(Err(err)) => Err(err),
            Err(_) => Err(Error::Ipc(format!(
                "Connection timeout after {:?}",
                self.retry_policy.connect_timeout
            ))),
        }
    }

    async fn try_connect_with_retries(
        &self,
        mut spawned_server: Option<&mut ServerProcess>,
    ) -> Result<Connection> {
        let mut last_error = None;

        if spawned_server.is_some() {
            debug!(
                "Polling for server readiness (timeout: {:?})",
                self.retry_policy.startup_poll_timeout
            );
            let start_time = tokio::time::Instant::now();
            let mut poll_interval = self.retry_policy.startup_initial_delay;
            while start_time.elapsed() < self.retry_policy.startup_poll_timeout {
                self.check_spawned_server(spawned_server.as_deref_mut())?;
                match self.try_connect().await {
                    Ok(connection) => {
                        info!("Connected to spawned server in {:?}", start_time.elapsed());
                        return Ok(connection);
                    }
                    Err(err) => {
                        last_error = Some(err);
                        self.check_spawned_server(spawned_server.as_deref_mut())?;
                        sleep(poll_interval).await;
                        poll_interval = self.retry_policy.next_startup_delay(poll_interval);
                    }
                }
            }
            debug!("Startup poll window elapsed; falling back to standard retries");
        }

        for attempt in 1..=self.retry_policy.connect_attempts {
            self.check_spawned_server(spawned_server.as_deref_mut())?;
            debug!(
                "Connection attempt {}/{}",
                attempt, self.retry_policy.connect_attempts
            );
            match self.try_connect().await {
                Ok(connection) => return Ok(connection),
                Err(err) => {
                    debug!("Connection attempt {} failed: {}", attempt, err);
                    last_error = Some(err);
                    self.check_spawned_server(spawned_server.as_deref_mut())?;
                    if attempt < self.retry_policy.connect_attempts {
                        sleep(self.retry_policy.connect_retry_delay).await;
                    }
                }
            }
        }

        Err(last_error.unwrap_or_else(|| {
            Error::Ipc("Failed to connect after all retry attempts".to_string())
        }))
    }

    fn check_spawned_server(&self, server: Option<&mut ServerProcess>) -> Result<()> {
        let Some(server) = server else {
            return Ok(());
        };
        match server.try_wait()? {
            Some(status) => Err(self.server_exited_before_connect(status)),
            None => Ok(()),
        }
    }

    fn server_exited_before_connect(&self, status: ExitStatus) -> Error {
        Error::ServerExitedBeforeConnect {
            socket_path: self.socket_path.clone(),
            status: status.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::process::ExitStatusExt;

    use super::*;

    #[test]
    fn default_retry_policy_preserves_existing_timing_contract() {
        let policy = ManagedServer::new_with_socket("/tmp/hotki.sock").retry_policy();

        assert_eq!(policy.startup_poll_timeout, Duration::from_millis(1000));
        assert_eq!(policy.startup_initial_delay, Duration::from_millis(10));
        assert_eq!(policy.startup_max_delay, Duration::from_millis(100));
        assert_eq!(policy.startup_delay_step, Duration::from_millis(10));
        assert_eq!(policy.connect_timeout, Duration::from_secs(5));
        assert_eq!(policy.connect_attempts, 5);
        assert_eq!(policy.connect_retry_delay, Duration::from_millis(200));
    }

    #[test]
    fn startup_poll_delay_caps_at_policy_maximum() {
        let policy = RetryPolicy::default();

        assert_eq!(
            policy.next_startup_delay(Duration::from_millis(10)),
            Duration::from_millis(20)
        );
        assert_eq!(
            policy.next_startup_delay(Duration::from_millis(100)),
            Duration::from_millis(100)
        );
    }

    #[test]
    fn server_exit_before_connect_error_is_typed() {
        let server = ManagedServer::new_with_socket("/tmp/hotki.sock");

        let Error::ServerExitedBeforeConnect {
            socket_path,
            status,
        } = server.server_exited_before_connect(ExitStatus::from_raw(7 << 8))
        else {
            panic!("expected typed early-exit error");
        };

        assert_eq!(socket_path, "/tmp/hotki.sock");
        assert!(status.contains('7'));
    }
}
