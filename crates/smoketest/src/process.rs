//! Process management utilities for smoketests.

use std::{
    env,
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
}

impl HelperWindowBuilder {
    /// Create a new helper window builder.
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            time_ms: 30000, // Default 30 seconds
        }
    }

    /// Set the lifetime of the helper window in milliseconds.
    pub fn with_time_ms(mut self, ms: u64) -> Self {
        self.time_ms = ms;
        self
    }

    /// Spawn the helper window process.
    pub fn spawn(self) -> Result<ManagedChild> {
        let exe = env::current_exe()?;

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
            "Failed to build hotki binary".to_string(),
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
