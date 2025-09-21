//! Process management utilities for smoketests.
use std::{
    env, fs,
    path::PathBuf,
    process as std_process,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant, SystemTime},
};

use crate::{
    config,
    error::{Error, Result},
    helper_window::ManagedChild,
};

/// Spawn the hands-off warning overlay (returns a managed child to kill later).
pub fn spawn_warn_overlay() -> Result<ManagedChild> {
    let exe = env::current_exe()?;
    let status_path = overlay_status_path_for_current_run();
    let info_path = overlay_info_path_for_current_run();
    let child = Command::new(exe)
        .env("HOTKI_SKIP_BUILD", "1")
        .arg("warn-overlay")
        .arg("--status-path")
        .arg(status_path)
        .arg("--info-path")
        .arg(info_path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| Error::SpawnFailed(e.to_string()))?;
    Ok(ManagedChild::new(child))
}

/// Spawn the warning overlay and wait the standard initial delay.
pub fn start_warn_overlay_with_delay() -> Option<ManagedChild> {
    reset_overlay_text();
    match spawn_warn_overlay() {
        Ok(child) => {
            thread::sleep(Duration::from_millis(config::WARN_OVERLAY.initial_delay_ms));
            Some(child)
        }
        Err(_) => None,
    }
}

/// Long-lived overlay instance used for multi-case smoketest runs.
pub struct OverlaySession {
    /// Child process hosting the hands-off overlay window.
    child: ManagedChild,
}

impl OverlaySession {
    /// Start the overlay and wait for the initial countdown to complete.
    pub fn start() -> Option<Self> {
        start_warn_overlay_with_delay().map(|child| Self { child })
    }

    /// Update the status line shown in the overlay window. Best-effort.
    pub fn set_status(&self, name: &str) {
        write_overlay_status(name);
    }

    /// Update the auxiliary info line in the overlay window. Best-effort.
    pub fn set_info(&self, info: &str) {
        write_overlay_info(info);
    }

    /// Shut down the overlay process.
    pub fn shutdown(mut self) -> Result<()> {
        self.child.kill_and_wait()
    }
}

/// Clear overlay text files so a fresh overlay starts without stale content.
pub fn reset_overlay_text() {
    let status_path = overlay_status_path_for_current_run();
    if fs::write(&status_path, b"").is_err() {
        // best effort, ignore failure
    }
    let info_path = overlay_info_path_for_current_run();
    if fs::write(&info_path, b"").is_err() {
        // best effort, ignore failure
    }
}

/// Compute the overlay status file path for the current smoketest run.
pub fn overlay_status_path_for_current_run() -> PathBuf {
    env::temp_dir().join(format!("hotki-smoketest-status-{}.txt", std_process::id()))
}

/// Compute the overlay info file path for the current smoketest run.
pub fn overlay_info_path_for_current_run() -> PathBuf {
    env::temp_dir().join(format!("hotki-smoketest-info-{}.txt", std_process::id()))
}

/// Write the current test name to the overlay status file. Best-effort.
pub fn write_overlay_status(name: &str) {
    let path = overlay_status_path_for_current_run();
    if let Err(_e) = fs::write(path, name.as_bytes()) {}
}

/// Write additional short info text for display in the overlay. Best-effort.
pub fn write_overlay_info(info: &str) {
    let path = overlay_info_path_for_current_run();
    if let Err(_e) = fs::write(path, info.as_bytes()) {}
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

/// Run a shell command and capture its output.
pub fn run_command(program: &str, args: &[&str]) -> Result<String> {
    let output = Command::new(program)
        .args(args)
        .output()
        .map_err(Error::Io)?;

    if !output.status.success() {
        return Err(Error::SpawnFailed(format!(
            "Command failed: {} {}",
            program,
            args.join(" ")
        )));
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Execute osascript (AppleScript) command.
pub fn osascript(script: &str) -> Result<String> {
    run_command("osascript", &["-e", script])
}
