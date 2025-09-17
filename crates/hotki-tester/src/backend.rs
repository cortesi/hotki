//! Backend process management utilities.

use std::{
    path::Path,
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use tracing::{debug, info, warn};

use crate::error::{Error, Result};

/// Handle for a spawned Hotki backend process.
pub struct BackendProcess {
    /// Handle to the spawned Hotki server process.
    child: Child,
    /// IPC socket path exposed by the backend.
    socket_path: String,
}

impl BackendProcess {
    /// Launch the Hotki binary in `--server` mode.
    pub fn spawn(bin: &Path, inherit_logs: bool, log_filter: Option<&str>) -> Result<Self> {
        info!(path = %bin.display(), "Spawning Hotki backend");
        let mut command = Command::new(bin);
        command.arg("--server");
        if let Some(filter) = log_filter {
            command.env("RUST_LOG", filter);
        }
        command.stdin(Stdio::null());
        if inherit_logs {
            command.stdout(Stdio::inherit());
            command.stderr(Stdio::inherit());
        } else {
            command.stdout(Stdio::null());
            command.stderr(Stdio::null());
        }
        let child = command.spawn()?;
        let socket_path = hotki_server::socket_path_for_pid(child.id());
        info!(pid = child.id(), socket = %socket_path, "Backend started");
        Ok(Self { child, socket_path })
    }

    /// Return the IPC socket path advertised by the backend.
    pub fn socket_path(&self) -> &str {
        &self.socket_path
    }

    /// Wait for the backend to exit gracefully.
    pub fn wait(mut self, timeout: Duration) -> Result<()> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(status) = self.child.try_wait()? {
                if status.success() {
                    info!("Backend exited cleanly");
                } else {
                    warn!(code = ?status.code(), "Backend exited with non-zero status");
                }
                return Ok(());
            }
            if Instant::now() >= deadline {
                warn!("Backend did not exit before timeout; sending SIGKILL");
                if let Err(err) = self.child.kill() {
                    warn!(?err, "Failed to kill backend child during timeout handling");
                }
                return Err(Error::other(format!(
                    "backend did not exit within {:?}",
                    timeout
                )));
            }
            thread::sleep(Duration::from_millis(100));
        }
    }

    /// Force-stop the backend, ignoring exit status.
    pub fn force_stop(&mut self) {
        if let Ok(None) = self.child.try_wait() {
            warn!(pid = self.child.id(), "Force-stopping backend process");
            if let Err(err) = self.child.kill() {
                warn!(?err, "Failed to kill backend child during force-stop");
            }
        }
    }
}

impl Drop for BackendProcess {
    fn drop(&mut self) {
        if let Ok(None) = self.child.try_wait() {
            debug!(
                pid = self.child.id(),
                "BackendProcess dropped with live child; killing"
            );
            if let Err(err) = self.child.kill() {
                warn!(?err, "Failed to kill backend child on drop");
            }
        }
    }
}
