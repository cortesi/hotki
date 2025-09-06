//! Process management utilities for smoketests.

use std::{
    env,
    path::PathBuf,
    process::{Child, Command, Stdio},
};

use crate::error::{Error, Result};

/// Managed child process that cleans up on drop.
pub struct ManagedChild {
    child: Option<Child>,
    pub pid: i32,
}

impl ManagedChild {
    /// Create a new managed child from a process.
    pub fn new(child: Child) -> Self {
        let pid = child.id() as i32;
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
        Ok(())
    }

    /// Check if the process is still running.
    pub fn is_running(&mut self) -> bool {
        if let Some(ref mut child) = self.child {
            match child.try_wait() {
                Ok(None) => true,  // Still running
                _ => false,        // Exited or error
            }
        } else {
            false
        }
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
    exe_path: Option<PathBuf>,
}

impl HelperWindowBuilder {
    /// Create a new helper window builder.
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            time_ms: 30000,  // Default 30 seconds
            exe_path: None,
        }
    }

    /// Set the lifetime of the helper window in milliseconds.
    pub fn with_time_ms(mut self, ms: u64) -> Self {
        self.time_ms = ms;
        self
    }

    /// Set a custom executable path (defaults to current exe).
    pub fn with_exe_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.exe_path = Some(path.into());
        self
    }

    /// Spawn the helper window process.
    pub fn spawn(self) -> Result<ManagedChild> {
        let exe = match self.exe_path {
            Some(path) => path,
            None => env::current_exe()?,
        };

        let child = Command::new(exe)
            .arg("focus-winhelper")
            .arg("--title")
            .arg(&self.title)
            .arg("--time")
            .arg(self.time_ms.to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| Error::SpawnFailed(e.to_string()))?;

        Ok(ManagedChild::new(child))
    }
}

/// Spawn a helper window process with the given title and lifetime.
/// This is a convenience function for simple cases.
pub fn spawn_helper_window(title: &str, time_ms: u64) -> Result<ManagedChild> {
    HelperWindowBuilder::new(title)
        .with_time_ms(time_ms)
        .spawn()
}

/// Build the hotki binary quietly.
/// Output is suppressed to avoid interleaved cargo logs.
pub fn build_hotki_quiet() -> Result<()> {
    let status = Command::new("cargo")
        .args(["build", "-q", "-p", "hotki"])
        .env("CARGO_TERM_COLOR", "never")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(Error::Io)?;

    if !status.success() {
        return Err(Error::SpawnFailed(
            "Failed to build hotki binary".to_string()
        ));
    }
    Ok(())
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

/// Take a screenshot using screencapture.
pub struct ScreenshotBuilder {
    rect: Option<(i32, i32, i32, i32)>,
    window_id: Option<u32>,
    path: PathBuf,
}

impl ScreenshotBuilder {
    /// Create a new screenshot builder.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            rect: None,
            window_id: None,
            path: path.into(),
        }
    }

    /// Capture a specific rectangle.
    pub fn with_rect(mut self, x: i32, y: i32, w: i32, h: i32) -> Self {
        self.rect = Some((x, y, w, h));
        self
    }

    /// Capture a specific window by ID.
    pub fn with_window_id(mut self, id: u32) -> Self {
        self.window_id = Some(id);
        self
    }

    /// Take the screenshot.
    pub fn capture(self) -> Result<()> {
        let mut cmd = Command::new("screencapture");
        cmd.arg("-x");  // No sound

        if let Some(id) = self.window_id {
            cmd.args(["-o", "-l", &id.to_string()]);
        } else if let Some((x, y, w, h)) = self.rect {
            let rect_str = format!("{},{},{},{}", x, y, w, h);
            cmd.args(["-R", &rect_str]);
        }

        cmd.arg(&self.path);

        let status = cmd.status().map_err(Error::Io)?;
        if !status.success() {
            return Err(Error::SpawnFailed("Screenshot capture failed".to_string()));
        }
        Ok(())
    }
}

/// Process cleanup guard that kills multiple processes on drop.
pub struct ProcessCleanupGuard {
    processes: Vec<ManagedChild>,
}

impl ProcessCleanupGuard {
    /// Create a new cleanup guard.
    pub fn new() -> Self {
        Self {
            processes: Vec::new(),
        }
    }

    /// Add a process to be cleaned up.
    pub fn add(&mut self, child: ManagedChild) {
        self.processes.push(child);
    }

    /// Take ownership of all processes (prevents automatic cleanup).
    pub fn take_all(&mut self) -> Vec<ManagedChild> {
        std::mem::take(&mut self.processes)
    }
}

impl Drop for ProcessCleanupGuard {
    fn drop(&mut self) {
        for mut proc in self.processes.drain(..) {
            let _ = proc.kill_and_wait();
        }
    }
}