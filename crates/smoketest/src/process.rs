//! Process management utilities for smoketests.
use std::{
    collections::HashSet,
    process::{Child, Command, ExitStatus},
    sync::{OnceLock, mpsc},
    thread,
    time::Duration,
};

use parking_lot::Mutex;

use crate::error::{Error, Result};

/// Global registry of helper process IDs for best-effort cleanup.
static PROCESS_REGISTRY: OnceLock<Mutex<HashSet<i32>>> = OnceLock::new();

/// Access the shared process registry, initializing it on first use.
fn registry() -> &'static Mutex<HashSet<i32>> {
    PROCESS_REGISTRY.get_or_init(|| Mutex::new(HashSet::new()))
}

/// Track a helper PID so watchdogs can terminate it later.
fn register_pid(pid: i32) {
    registry().lock().insert(pid);
}

/// Remove a helper PID from the registry once it has exited cleanly.
fn unregister_pid(pid: i32) {
    registry().lock().remove(&pid);
}

/// Kill all registered processes (best-effort cleanup).
pub fn kill_all() {
    let pids: Vec<i32> = registry().lock().iter().copied().collect();
    for pid in pids {
        unsafe {
            let _ = libc::kill(pid as libc::pid_t, libc::SIGKILL);
        }
    }
}

/// Managed child process that cleans up on drop.
pub struct ManagedChild {
    /// Handle to the spawned child process.
    child: Option<Child>,
    /// Process identifier for the child.
    pub pid: i32,
}

impl ManagedChild {
    /// Wrap a child process and register it for bookkeeping.
    pub fn new(child: Child) -> Self {
        let pid = child.id() as i32;
        register_pid(pid);
        Self {
            child: Some(child),
            pid,
        }
    }

    /// Terminate the child and wait for it to exit.
    pub fn kill_and_wait(&mut self) -> Result<()> {
        if let Some(mut child) = self.child.take() {
            let kill_result = child.kill();
            let wait_result = child.wait();
            unregister_pid(self.pid);
            kill_result.map_err(Error::Io)?;
            wait_result.map_err(Error::Io)?;
            return Ok(());
        }
        unregister_pid(self.pid);
        Ok(())
    }

    /// Wait up to `remaining` for exit, then force termination and reap the child.
    pub fn wait_with_budget(&mut self, remaining: Duration) -> Result<(ExitStatus, bool)> {
        let mut child = self.child.take().ok_or_else(|| {
            Error::InvalidState(format!("child process {} is no longer owned", self.pid))
        })?;
        let (tx_status, rx_status) = mpsc::sync_channel(1);
        let waiter = thread::Builder::new()
            .name(format!("hotki-child-wait-{}", self.pid))
            .spawn(move || {
                if let Err(error) = tx_status.send(child.wait()) {
                    tracing::debug!(?error, "child wait receiver disconnected");
                }
            });
        if let Err(error) = waiter {
            force_reap_pid(self.pid);
            unregister_pid(self.pid);
            return Err(Error::Io(error));
        }

        let result = match rx_status.recv_timeout(remaining) {
            Ok(status) => status.map(|status| (status, false)).map_err(Error::Io),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                unsafe {
                    let _ = libc::kill(self.pid as libc::pid_t, libc::SIGKILL);
                }
                match rx_status.recv() {
                    Ok(status) => status.map(|status| (status, true)).map_err(Error::Io),
                    Err(error) => {
                        force_reap_pid(self.pid);
                        Err(Error::InvalidState(format!(
                            "child process {} waiter disconnected after forced termination: \
                             {error}",
                            self.pid
                        )))
                    }
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                force_reap_pid(self.pid);
                Err(Error::InvalidState(format!(
                    "child process {} waiter disconnected",
                    self.pid
                )))
            }
        };
        unregister_pid(self.pid);
        result
    }
}

/// Force a process to exit and reap it when its owned `Child` waiter was lost.
fn force_reap_pid(pid: i32) {
    unsafe {
        let _ = libc::kill(pid as libc::pid_t, libc::SIGKILL);
        let mut status = 0;
        let _ = libc::waitpid(pid as libc::pid_t, &mut status, 0);
    }
}

impl Drop for ManagedChild {
    fn drop(&mut self) {
        if self.kill_and_wait().is_err() {
            // best-effort cleanup on drop
        }
    }
}

/// Spawn the provided command and wrap it in a [`ManagedChild`].
pub fn spawn_managed(mut cmd: Command) -> Result<ManagedChild> {
    let child = cmd.spawn().map_err(|e| Error::SpawnFailed(e.to_string()))?;
    Ok(ManagedChild::new(child))
}

/// Ask Cargo to build the current hotki app and report its diagnostics directly.
pub fn build_hotki_app() -> Result<()> {
    let mut command = Command::new("cargo");
    command.args(["build", "-p", "hotki-app", "--bin", "hotki-app"]);
    if !cfg!(debug_assertions) {
        command.arg("--release");
    }
    let status = command.status().map_err(Error::Io)?;
    if status.success() {
        Ok(())
    } else {
        Err(Error::SpawnFailed(format!(
            "cargo build -p hotki-app --bin hotki-app exited with {status}"
        )))
    }
}

#[cfg(test)]
mod tests {
    use std::process::Stdio;

    use super::*;

    #[test]
    fn exhausted_wait_budget_forces_and_reaps_child() {
        let mut command = Command::new("sh");
        command
            .args(["-c", "read _"])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let mut child = spawn_managed(command).expect("spawn blocking child");

        let (_status, forced) = child
            .wait_with_budget(Duration::ZERO)
            .expect("force and reap child");

        assert!(forced);
        assert!(!registry().lock().contains(&child.pid));
    }
}
