//! Process management utilities for smoketests.
use std::{
    collections::HashSet,
    process::{Child, Command},
    sync::OnceLock,
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
            child.kill().map_err(Error::Io)?;
            child.wait().map_err(Error::Io)?;
        }
        unregister_pid(self.pid);
        Ok(())
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
