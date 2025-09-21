//! Repeat throughput smoketest cases executed via the registry runner.
use std::{
    cmp::max,
    env, fs,
    process::{Command, Stdio},
};

use crate::{
    config,
    error::{Error, Result},
    suite::{CaseCtx, StageHandle},
};

/// Verify relay repeat throughput using the mimic-driven runner.
pub fn repeat_relay_throughput(ctx: &mut CaseCtx<'_>) -> Result<()> {
    let duration_ms = config::DEFAULTS.duration_ms;
    run_repeat_case(ctx, "repeat-relay", duration_ms)
}

/// Verify shell repeat throughput using the mimic-driven runner.
pub fn repeat_shell_throughput(ctx: &mut CaseCtx<'_>) -> Result<()> {
    let duration_ms = config::DEFAULTS.duration_ms;
    run_repeat_case(ctx, "repeat-shell", duration_ms)
}

/// Verify system volume repeat throughput using the mimic-driven runner.
pub fn repeat_volume_throughput(ctx: &mut CaseCtx<'_>) -> Result<()> {
    let duration_ms = max(
        config::DEFAULTS.duration_ms,
        config::DEFAULTS.min_volume_duration_ms,
    );
    run_repeat_case(ctx, "repeat-volume", duration_ms)
}

/// Shared harness that runs a repeat counting routine in a subprocess and emits artifacts.
fn run_repeat_case(ctx: &mut CaseCtx<'_>, slug: &str, duration_ms: u64) -> Result<()> {
    ctx.setup(|_| Ok(()))?;
    let output = ctx.action(|_| run_repeat_subprocess(slug, duration_ms))?;
    ctx.settle(|stage| record_repeat_stats(stage, slug, duration_ms, &output))?;
    Ok(())
}

/// Execute the legacy repeat smoketest command as a subprocess and capture its output.
fn run_repeat_subprocess(slug: &str, duration_ms: u64) -> Result<RepeatOutput> {
    let exe = env::current_exe()?;
    let duration_arg = duration_ms.to_string();
    let output = Command::new(exe)
        .arg("--quiet")
        .arg("--no-warn")
        .arg("--duration")
        .arg(&duration_arg)
        .arg(slug)
        .env("HOTKI_SKIP_BUILD", "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()?;

    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();

    if !output.status.success() {
        return Err(Error::InvalidState(format!(
            "repeat command '{slug}' failed: status={} stderr={stderr}",
            output.status
        )));
    }

    let repeats = parse_repeat_count(&stdout).ok_or_else(|| {
        Error::InvalidState(format!(
            "repeat command '{slug}' did not emit a repeat count: stdout={stdout:?}"
        ))
    })?;

    Ok(RepeatOutput {
        repeats,
        stdout,
        stderr,
    })
}

/// Persist repeat metrics and captured output to artifact files for later inspection.
fn record_repeat_stats(
    stage: &mut StageHandle<'_>,
    slug: &str,
    duration_ms: u64,
    output: &RepeatOutput,
) -> Result<()> {
    let sanitized = slug.replace('-', "_");
    let stats_path = stage
        .artifacts_dir()
        .join(format!("{}_stats.txt", sanitized));
    let stats_contents = format!(
        "case={slug}\nduration_ms={duration_ms}\nrepeats={}\n",
        output.repeats
    );
    fs::write(&stats_path, stats_contents)?;
    stage.record_artifact(&stats_path);

    let log_path = stage
        .artifacts_dir()
        .join(format!("{}_output.log", sanitized));
    let log_contents = format!("stdout:\n{}\n\nstderr:\n{}\n", output.stdout, output.stderr);
    fs::write(&log_path, log_contents)?;
    stage.record_artifact(&log_path);

    Ok(())
}

/// Extract the repeat count from legacy command stdout.
fn parse_repeat_count(stdout: &str) -> Option<usize> {
    stdout.lines().rev().find_map(|line| {
        let line = line.trim();
        let suffix = " repeats";
        if let Some(idx) = line.find(suffix) {
            let number_part = line[..idx].trim();
            return number_part.parse::<usize>().ok();
        }
        None
    })
}

/// Captured output and parsed repeat metrics from the legacy command.
struct RepeatOutput {
    /// Total number of repeats observed during the subprocess run.
    repeats: usize,
    /// Captured standard output from the legacy command.
    stdout: String,
    /// Captured standard error from the legacy command.
    stderr: String,
}
