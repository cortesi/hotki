//! Process management utilities for smoketests.
use std::{
    env, fs,
    path::PathBuf,
    process as std_process,
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant, SystemTime},
};

use crate::{
    config,
    error::{Error, Result},
    proc_registry,
};

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
        proc_registry::register(pid);
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
        proc_registry::unregister(self.pid);
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
    let status_path = overlay_status_path_for_current_run();
    let info_path = overlay_info_path_for_current_run();
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
        let status_path = overlay_status_path_for_current_run();
        if fs::write(&status_path, b"").is_err() {
            // best effort, ignore failure
        }
        let info_path = overlay_info_path_for_current_run();
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
        write_overlay_text("status", name);
    }

    /// Update the auxiliary info line in the overlay window. Best-effort.
    pub fn set_info(&self, info: &str) {
        write_overlay_text("info", info);
    }
}

impl Drop for OverlaySession {
    fn drop(&mut self) {
        if let Err(e) = self.child.kill_and_wait() {
            eprintln!("overlay: failed to stop helper: {}", e);
        }
    }
}

/// Compute the overlay status file path for the current smoketest run.
pub fn overlay_status_path_for_current_run() -> PathBuf {
    overlay_path_for_current_run("status")
}

/// Compute the overlay info file path for the current smoketest run.
pub fn overlay_info_path_for_current_run() -> PathBuf {
    overlay_path_for_current_run("info")
}

/// Compute an overlay file path for the current smoketest run.
fn overlay_path_for_current_run(label: &str) -> PathBuf {
    env::temp_dir().join(format!("hotki-smoketest-{label}-{}.txt", std_process::id()))
}

/// Write overlay text to the temp file for the requested label.
fn write_overlay_text(label: &str, text: &str) {
    let path = overlay_path_for_current_run(label);
    if let Err(_e) = fs::write(path, text.as_bytes()) {}
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
