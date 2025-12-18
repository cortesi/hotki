//! Utilities for running external commands.

use std::{
    ffi::OsStr,
    path::Path,
    process::{Command, Stdio},
};

use crate::{Error, Result};

/// Run a command inheriting stdio and return an error on non-zero exit.
pub fn run_status_streaming<I, S>(cwd: &Path, program: &str, args: I) -> Result<()>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let status = Command::new(program)
        .args(args)
        .current_dir(cwd)
        .status()
        .map_err(|source| Error::CommandStart {
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

/// Run a command with stdout/stderr suppressed and return an error on non-zero exit.
pub fn run_status_quiet<I, S>(cwd: &Path, program: &str, args: I) -> Result<()>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let status = Command::new(program)
        .args(args)
        .current_dir(cwd)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|source| Error::CommandStart {
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
