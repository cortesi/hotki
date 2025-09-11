//! Process management utilities for smoketests.

use std::{
    env,
    path::PathBuf,
    process::{Child, Command, Stdio},
    thread,
    time::Duration,
};

use crate::{
    error::{Error, Result},
    proc_registry,
};

/// Managed child process that cleans up on drop.
pub struct ManagedChild {
    child: Option<Child>,
    pub pid: i32,
}

impl ManagedChild {
    /// Create a new managed child from a process.
    pub fn new(child: Child) -> Self {
        let pid = child.id() as i32;
        proc_registry::register(pid);
        Self {
            child: Some(child),
            pid,
        }
    }

    /// Kill the process and wait for it to exit.
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
        let _ = self.kill_and_wait();
    }
}

/// Builder for spawning helper windows with common configurations.
pub struct HelperWindowBuilder {
    title: String,
    time_ms: u64,
    grid: Option<(u32, u32, u32, u32)>,
    size: Option<(f64, f64)>,
    pos: Option<(f64, f64)>,
    label_text: Option<String>,
}

impl HelperWindowBuilder {
    /// Create a new helper window builder.
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            time_ms: 30000, // Default 30 seconds
            grid: None,
            size: None,
            pos: None,
            label_text: None,
        }
    }

    /// Set the lifetime of the helper window in milliseconds.
    pub fn with_time_ms(mut self, ms: u64) -> Self {
        self.time_ms = ms;
        self
    }

    // with_slot intentionally omitted in favor of with_grid in tests.

    /// Place into an arbitrary grid cell (top-left origin)
    pub fn with_grid(mut self, cols: u32, rows: u32, col: u32, row: u32) -> Self {
        self.grid = Some((cols, rows, col, row));
        self
    }

    /// Set requested window size
    pub fn with_size(mut self, width: f64, height: f64) -> Self {
        self.size = Some((width, height));
        self
    }

    /// Set requested window position (x, y)
    pub fn with_position(mut self, x: f64, y: f64) -> Self {
        self.pos = Some((x, y));
        self
    }

    /// Set explicit label text to display
    pub fn with_label_text(mut self, text: impl Into<String>) -> Self {
        self.label_text = Some(text.into());
        self
    }

    /// Spawn the helper window process.
    pub fn spawn(self) -> Result<ManagedChild> {
        let exe = env::current_exe()?;

        let mut cmd = Command::new(exe);
        cmd.arg("focus-winhelper")
            .arg("--title")
            .arg(&self.title)
            .arg("--time")
            .arg(self.time_ms.to_string());
        if let Some((c, r, col, row)) = self.grid {
            cmd.arg("--grid").args([
                c.to_string(),
                r.to_string(),
                col.to_string(),
                row.to_string(),
            ]);
        }
        if let Some((w, h)) = self.size {
            cmd.arg("--size").args([w.to_string(), h.to_string()]);
        }
        if let Some((x, y)) = self.pos {
            cmd.arg("--pos").args([x.to_string(), y.to_string()]);
        }
        if let Some(ref txt) = self.label_text {
            cmd.arg("--label-text").arg(txt);
        }
        let child = cmd
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| Error::SpawnFailed(e.to_string()))?;

        Ok(ManagedChild::new(child))
    }
}

/// Spawn the hands-off warning overlay (returns a managed child to kill later).
pub fn spawn_warn_overlay() -> Result<ManagedChild> {
    let exe = env::current_exe()?;
    let status_path = overlay_status_path_for_current_run();
    let child = Command::new(exe)
        .arg("warn-overlay")
        .arg("--status-path")
        .arg(status_path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| Error::SpawnFailed(e.to_string()))?;
    Ok(ManagedChild::new(child))
}

/// Spawn the warning overlay and wait the standard initial delay.
pub fn start_warn_overlay_with_delay() -> Option<ManagedChild> {
    match spawn_warn_overlay() {
        Ok(child) => {
            thread::sleep(Duration::from_millis(
                crate::config::WARN_OVERLAY_INITIAL_DELAY_MS,
            ));
            Some(child)
        }
        Err(_) => None,
    }
}

/// Compute the overlay status file path for the current smoketest run.
pub fn overlay_status_path_for_current_run() -> PathBuf {
    std::env::temp_dir().join(format!("hotki-smoketest-status-{}.txt", std::process::id()))
}

/// Write the current test name to the overlay status file. Best-effort.
pub fn write_overlay_status(name: &str) {
    let path = overlay_status_path_for_current_run();
    let _ = std::fs::write(path, name.as_bytes());
}

/// Build the hotki binary quietly.
/// Output is suppressed to avoid interleaved cargo logs.
pub fn build_hotki_quiet() -> Result<()> {
    // First check if the binary already exists and is recent
    if let Ok(metadata) = std::fs::metadata("target/debug/hotki")
        && let Ok(modified) = metadata.modified()
        && let Ok(elapsed) = std::time::SystemTime::now().duration_since(modified)
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
    let timeout = std::time::Duration::from_secs(60);
    let start = std::time::Instant::now();

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
                    let _ = child.kill();
                    return Err(Error::SpawnFailed(
                        "Build timeout: cargo build took too long".to_string(),
                    ));
                }
                std::thread::sleep(std::time::Duration::from_millis(
                    crate::config::RETRY_DELAY_MS,
                ));
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
