//! Repeat throughput smoketest cases executed via the registry runner.
use std::{
    cmp::max,
    env, fs,
    process::{self as std_process, Command},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    thread,
    time::{SystemTime, UNIX_EPOCH},
};

use hotki_engine::Repeater;
use hotki_protocol::ipc;
use parking_lot::Mutex;
use tokio::runtime::Builder;

use crate::{
    config,
    error::{Error, Result},
    suite::CaseCtx,
};

/// Identifier used for relay repeat runs.
const RELAY_APP_ID: &str = "smoketest-relay";
/// Identifier used for shell repeat runs.
const SHELL_APP_ID: &str = "smoketest-shell";
/// Identifier used for volume repeat runs.
const VOLUME_APP_ID: &str = "smoketest-volume";

/// Verify relay repeat throughput using a lightweight in-process harness.
pub fn repeat_relay_throughput(ctx: &mut CaseCtx<'_>) -> Result<()> {
    let duration_ms = config::DEFAULTS.duration_ms;
    run_repeat_case(ctx, "repeat-relay", duration_ms)
}

/// Verify shell repeat throughput using a simple tail file.
pub fn repeat_shell_throughput(ctx: &mut CaseCtx<'_>) -> Result<()> {
    let duration_ms = config::DEFAULTS.duration_ms;
    run_repeat_case(ctx, "repeat-shell", duration_ms)
}

/// Verify system volume repeat throughput using AppleScript.
pub fn repeat_volume_throughput(ctx: &mut CaseCtx<'_>) -> Result<()> {
    let duration_ms = max(
        config::DEFAULTS.duration_ms,
        config::DEFAULTS.min_volume_duration_ms,
    );
    run_repeat_case(ctx, "repeat-volume", duration_ms)
}

/// Shared harness that runs a repeat counting routine in-process and logs diagnostics.
fn run_repeat_case(ctx: &mut CaseCtx<'_>, slug: &str, duration_ms: u64) -> Result<()> {
    ctx.setup(|_| Ok(()))?;
    let output = ctx.action(|_| run_repeat_workload(slug, duration_ms))?;
    ctx.settle(|ctx| record_repeat_stats(ctx, slug, duration_ms, &output))?;
    Ok(())
}

/// Execute the repeat workload directly for the supplied slug.
fn run_repeat_workload(slug: &str, duration_ms: u64) -> Result<RepeatOutput> {
    let repeats = match slug {
        "repeat-relay" => count_relay(duration_ms)?,
        "repeat-shell" => count_shell(duration_ms)?,
        "repeat-volume" => count_volume(duration_ms)?,
        _ => return Err(Error::InvalidState(format!("unknown repeat slug: {slug}"))),
    };

    let stdout = format!("{repeats} repeats\n");
    Ok(RepeatOutput {
        repeats,
        stdout,
        stderr: String::new(),
    })
}

/// Log repeat metrics and captured output for later inspection.
fn record_repeat_stats(
    ctx: &CaseCtx<'_>,
    slug: &str,
    duration_ms: u64,
    output: &RepeatOutput,
) -> Result<()> {
    ctx.log_event(
        "repeat_stats",
        &format!(
            "slug={slug} duration_ms={duration_ms} repeats={}",
            output.repeats
        ),
    );
    ctx.log_event(
        "repeat_output",
        &format!("stdout={:?} stderr={:?}", output.stdout, output.stderr),
    );
    Ok(())
}

/// Count relay repeats for the configured duration.
fn count_relay(duration_ms: u64) -> Result<usize> {
    let runtime = Builder::new_multi_thread()
        .enable_time()
        .build()
        .map_err(|e| Error::InvalidState(format!("failed to build tokio runtime: {e}")))?;
    let _guard = runtime.enter();

    let focus_ctx = Arc::new(Mutex::new(None::<(String, String, i32)>));
    let relay = hotki_engine::RelayHandler::new();
    let (tx, _rx) = ipc::ui_channel();
    let notifier = hotki_engine::NotificationDispatcher::new(tx);
    let repeater = hotki_engine::Repeater::new_with_ctx(focus_ctx.clone(), relay, notifier);

    let counter = Arc::new(AtomicUsize::new(0));
    let counter2 = counter.clone();
    repeater.set_on_relay_repeat(Arc::new(move |_id| {
        counter2.fetch_add(1, Ordering::SeqCst);
    }));

    let chord = mac_keycode::Chord::parse("right")
        .or_else(|| mac_keycode::Chord::parse("a"))
        .ok_or_else(|| Error::InvalidState("failed to parse repeat chord".into()))?;

    let timeout = config::ms(duration_ms);
    let title = config::test_title("relay");
    let id = RELAY_APP_ID.to_string();

    {
        let mut guard = focus_ctx.lock();
        *guard = Some(("smoketest-app".into(), title, std_process::id() as i32));
    }

    repeater.start_relay_repeat(id.clone(), chord, Some(hotki_engine::RepeatSpec::default()));

    thread::sleep(timeout);
    shutdown_repeater(&repeater, &id)?;

    Ok(counter.load(Ordering::SeqCst))
}

/// Stop a repeater ticker and verify it fully drained.
fn shutdown_repeater(repeater: &Repeater, id: &str) -> Result<()> {
    repeater.stop_sync(id);
    repeater.clear_sync();
    if repeater.is_ticking(id) {
        return Err(Error::InvalidState(format!(
            "repeat '{id}' still ticking after shutdown"
        )));
    }
    Ok(())
}

/// Count shell repeats by tailing a temporary file.
fn count_shell(duration_ms: u64) -> Result<usize> {
    let runtime = Builder::new_multi_thread()
        .enable_time()
        .build()
        .map_err(|e| Error::InvalidState(format!("failed to build tokio runtime: {e}")))?;
    let _guard = runtime.enter();

    let focus_ctx = Arc::new(Mutex::new(None::<(String, String, i32)>));
    let relay = hotki_engine::RelayHandler::new();
    let (tx, _rx) = ipc::ui_channel();
    let notifier = hotki_engine::NotificationDispatcher::new(tx);
    let repeater = hotki_engine::Repeater::new_with_ctx(focus_ctx, relay, notifier);

    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| Error::InvalidState(format!("system clock error: {e}")))?;
    let path = env::temp_dir().join(format!(
        "hotki-smoketest-shell-{}-{}.log",
        std_process::id(),
        timestamp.as_nanos()
    ));

    fs::File::create(&path)?;
    let cmd = format!("printf . >> {}", sh_single_quote(&path.to_string_lossy()));

    let id = SHELL_APP_ID.to_string();
    repeater.start_shell_repeat(id.clone(), cmd, Some(hotki_engine::RepeatSpec::default()));
    thread::sleep(config::ms(duration_ms));
    repeater.stop_sync(&id);

    let repeats = match fs::read(&path) {
        Ok(bytes) => bytes.len().saturating_sub(1),
        Err(err) => {
            tracing::debug!(
                "repeat-shell: failed to read repeat log {}: {}",
                path.display(),
                err
            );
            0
        }
    };
    if let Err(err) = fs::remove_file(&path) {
        tracing::debug!(
            "repeat-shell: failed to remove repeat log {}: {}",
            path.display(),
            err
        );
    }
    shutdown_repeater(&repeater, &id)?;
    Ok(repeats)
}

/// Count volume repeats using AppleScript and restore the original level.
fn count_volume(duration_ms: u64) -> Result<usize> {
    let runtime = Builder::new_multi_thread()
        .enable_time()
        .build()
        .map_err(|e| Error::InvalidState(format!("failed to build tokio runtime: {e}")))?;
    let _guard = runtime.enter();

    let original_volume = get_volume().unwrap_or(50);
    if let Err(err) = set_volume_abs(0) {
        tracing::debug!("repeat-volume: failed to zero volume: {}", err);
    }

    let script = "set currentVolume to output volume of (get volume settings)\nset volume output volume (currentVolume + 1)";
    let command = format!("osascript -e '{}'", script.replace('\n', "' -e '"));

    let focus_ctx = Arc::new(Mutex::new(None::<(String, String, i32)>));
    let relay = hotki_engine::RelayHandler::new();
    let (tx, _rx) = ipc::ui_channel();
    let notifier = hotki_engine::NotificationDispatcher::new(tx);
    let repeater = Repeater::new_with_ctx(focus_ctx, relay, notifier);

    let id = VOLUME_APP_ID.to_string();
    repeater.start_shell_repeat(
        id.clone(),
        command,
        Some(hotki_engine::RepeatSpec::default()),
    );
    thread::sleep(config::ms(duration_ms));
    repeater.stop_sync(&id);
    shutdown_repeater(&repeater, &id)?;

    let volume = get_volume().unwrap_or(0);
    let repeats = volume.saturating_sub(1) as usize;
    match set_volume_abs(original_volume as u8) {
        Ok(()) => match get_volume() {
            Some(current) if current == original_volume => {}
            Some(current) => {
                return Err(Error::InvalidState(format!(
                    "expected volume {} after restore, observed {}",
                    original_volume, current
                )));
            }
            None => {
                return Err(Error::InvalidState(
                    "failed to read volume after restoration".into(),
                ));
            }
        },
        Err(err) => {
            return Err(Error::InvalidState(format!(
                "failed to restore volume: {err}"
            )));
        }
    }
    Ok(repeats)
}

/// Run an AppleScript command and capture stdout.
fn osascript(script: &str) -> Result<String> {
    let output = Command::new("osascript").arg("-e").arg(script).output()?;

    if !output.status.success() {
        return Err(Error::InvalidState(format!(
            "osascript exited with status {status}: {stderr}",
            status = output.status,
            stderr = String::from_utf8_lossy(&output.stderr)
        )));
    }

    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Get the current output volume percentage via AppleScript.
fn get_volume() -> Option<u64> {
    let out = osascript("output volume of (get volume settings)").ok()?;
    out.trim().parse::<u64>().ok()
}

/// Set the output volume to an absolute level [0,100].
fn set_volume_abs(level: u8) -> Result<()> {
    let cmd = format!("set volume output volume {}", level.min(100));
    let _ = osascript(&cmd)?;
    Ok(())
}

/// Return a shell single-quoted string escaping embedded quotes safely.
fn sh_single_quote(s: &str) -> String {
    let mut out = String::from("'");
    for ch in s.chars() {
        if ch == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(ch);
        }
    }
    out.push('\'');
    out
}

/// Captured output and parsed repeat metrics from the repeat workload.
struct RepeatOutput {
    /// Total number of repeats observed during the run.
    repeats: usize,
    /// Captured standard output emulating the legacy command.
    stdout: String,
    /// Captured standard error (unused for now).
    stderr: String,
}
