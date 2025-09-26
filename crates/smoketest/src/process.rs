//! Process management utilities for smoketests.
use std::{
    collections::HashSet,
    env, fs,
    process::{Child, Command, Stdio},
    sync::OnceLock,
    thread,
    time::{Duration, Instant, SystemTime},
};

use parking_lot::Mutex;

use crate::{
    config,
    error::{Error, Result},
    warn_overlay,
};

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

/// Internal flag passed to the smoketest binary to start the warn overlay helper.
pub const WARN_OVERLAY_STANDALONE_FLAG: &str = "--hotki-internal-warn-overlay";

/// Spawn the hands-off warning overlay (returns a managed child to kill later).
pub fn spawn_warn_overlay() -> Result<ManagedChild> {
    let exe = env::current_exe()?;
    let status_path = warn_overlay::status_file_path();
    let info_path = warn_overlay::info_file_path();
    let mut cmd = Command::new(exe);
    cmd.env("HOTKI_SKIP_BUILD", "1")
        .arg(WARN_OVERLAY_STANDALONE_FLAG)
        .arg("--status-path")
        .arg(status_path)
        .arg("--info-path")
        .arg(info_path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let child = spawn_managed(cmd)?;
    Ok(child)
}

/// Long-lived overlay instance used for multi-case smoketest runs.
pub struct OverlaySession {
    /// Child process hosting the hands-off overlay window.
    child: ManagedChild,
}

impl OverlaySession {
    /// Start the overlay and wait for the initial countdown to complete.
    pub fn start() -> Option<Self> {
        let status_path = warn_overlay::status_file_path();
        if fs::write(&status_path, b"").is_err() {
            // best effort, ignore failure
        }
        let info_path = warn_overlay::info_file_path();
        if fs::write(&info_path, b"").is_err() {
            // best effort, ignore failure
        }
        match spawn_warn_overlay() {
            Ok(child) => {
                thread::sleep(Duration::from_millis(config::WARN_OVERLAY.initial_delay_ms));
                Some(Self { child })
            }
            Err(_) => None,
        }
    }

    /// Update the status line shown in the overlay window. Best-effort.
    pub fn set_status(&self, name: &str) {
        warn_overlay::write_overlay_text("status", name);
    }

    /// Update the auxiliary info line in the overlay window. Best-effort.
    pub fn set_info(&self, info: &str) {
        warn_overlay::write_overlay_text("info", info);
    }
}

impl Drop for OverlaySession {
    fn drop(&mut self) {
        if let Err(e) = self.child.kill_and_wait() {
            eprintln!("overlay: failed to stop helper: {}", e);
        }
    }
}

/// Build the hotki binary quietly.
/// Output is suppressed to avoid interleaved cargo logs.
pub fn build_hotki_quiet() -> Result<()> {
    // First check if the binary already exists and is recent
    if let Ok(metadata) = fs::metadata("target/debug/hotki")
        && let Ok(modified) = metadata.modified()
        && let Ok(elapsed) = SystemTime::now().duration_since(modified)
        && elapsed.as_secs() < 60
    {
        // If binary was built in the last 60 seconds, skip rebuild
        return Ok(());
    }

    let mut child = Command::new("cargo")
        .args(["build", "-q", "-p", "hotki"])
        .env("CARGO_TERM_COLOR", "never")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(Error::Io)?;

    // Wait for up to 60 seconds
    let timeout = Duration::from_secs(60);
    let start = Instant::now();

    loop {
        match child.try_wait().map_err(Error::Io)? {
            Some(status) => {
                if !status.success() {
                    return Err(Error::SpawnFailed(
                        "Failed to build hotki binary".to_string(),
                    ));
                }
                return Ok(());
            }
            None => {
                if start.elapsed() > timeout {
                    if let Err(_e) = child.kill() {}
                    return Err(Error::SpawnFailed(
                        "Build timeout: cargo build took too long".to_string(),
                    ));
                }
                thread::sleep(Duration::from_millis(config::INPUT_DELAYS.retry_delay_ms));
            }
        }
    }
}
