//! Utilities for running external commands.

use std::{
    ffi::OsStr,
    path::Path,
    process::{Command, Stdio},
};

use crate::{Error, Result};

/// Output handling mode for spawned commands.
#[derive(Clone, Copy, Debug)]
pub enum OutputMode {
    /// Inherit stdout/stderr from the current process.
    Streaming,
    /// Suppress stdout/stderr unless the command fails.
    Quiet,
}

/// Run a command and return an error on non-zero exit.
pub fn run_status<I, S>(cwd: &Path, program: &str, args: I, output: OutputMode) -> Result<()>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut command = Command::new(program);
    command.args(args).current_dir(cwd);
    if matches!(output, OutputMode::Quiet) {
        command.stdout(Stdio::null()).stderr(Stdio::null());
    }
    let status = command.status().map_err(|source| Error::CommandStart {
        program: program.to_string(),
        source,
    })?;
    if status.success() {
        Ok(())
    } else {
        Err(Error::CommandFailed {
            program: program.to_string(),
            status,
        })
    }
}
