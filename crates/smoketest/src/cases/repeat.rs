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
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use hotki_engine::{RepeatObserver, Repeater};
use hotki_protocol::ipc;
use hotki_world::mimic::{EventLoopHandle, shared_event_loop};
use parking_lot::Mutex;
use tokio::runtime::Builder;
use winit::{
    application::ApplicationHandler,
    event::WindowEvent,
    event_loop::{ActiveEventLoop, ControlFlow},
    platform::pump_events::PumpStatus,
    window::{Window, WindowId},
};

use crate::{
    config,
    error::{Error, Result},
    suite::{CaseCtx, StageHandle},
};

/// Identifier used for relay repeat runs.
const RELAY_APP_ID: &str = "smoketest-relay";
/// Identifier used for shell repeat runs.
const SHELL_APP_ID: &str = "smoketest-shell";
/// Identifier used for volume repeat runs.
const VOLUME_APP_ID: &str = "smoketest-volume";
/// Width of the helper window used in relay repeats.
const RELAY_WINDOW_WIDTH: f64 = 280.0;
/// Height of the helper window used in relay repeats.
const RELAY_WINDOW_HEIGHT: f64 = 180.0;
/// Margin applied when positioning the relay helper window.
const RELAY_WINDOW_MARGIN: f64 = 8.0;

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

/// Shared harness that runs a repeat counting routine in-process and emits artifacts.
fn run_repeat_case(ctx: &mut CaseCtx<'_>, slug: &str, duration_ms: u64) -> Result<()> {
    ctx.setup(|_| Ok(()))?;
    let output = ctx.action(|_| run_repeat_workload(slug, duration_ms))?;
    ctx.settle(|stage| record_repeat_stats(stage, slug, duration_ms, &output))?;
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

    let counter = Arc::new(RelayCounter(AtomicUsize::new(0)));
    repeater.set_repeat_observer(counter.clone());

    let chord = mac_keycode::Chord::parse("right")
        .or_else(|| mac_keycode::Chord::parse("a"))
        .ok_or_else(|| Error::InvalidState("failed to parse repeat chord".into()))?;

    let timeout = config::ms(duration_ms);
    let title = config::test_title("relay");
    {
        let mut focus = focus_ctx.lock();
        *focus = Some((
            "smoketest-app".to_string(),
            title.clone(),
            std_process::id() as i32,
        ));
    }
    let mut app = RelayApp {
        repeater,
        window: None,
        id: RELAY_APP_ID.into(),
        chord,
        started: false,
        start: None,
        timeout,
        title,
        finished: false,
    };

    let handle = shared_event_loop();
    run_relay_loop(&handle, &mut app)?;

    Ok(counter.0.load(Ordering::SeqCst))
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

    let volume = get_volume().unwrap_or(0);
    let repeats = volume.saturating_sub(1) as usize;
    if let Err(err) = set_volume_abs(original_volume as u8) {
        tracing::debug!("repeat-volume: failed to restore volume: {}", err);
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

/// Minimal repeat observer that counts relay repeats.
struct RelayCounter(AtomicUsize);

impl RepeatObserver for RelayCounter {
    fn on_relay_repeat(&self, _id: &str) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

/// Minimal winit application used for relay repeat measurement.
struct RelayApp {
    /// Repeater used to drive input events.
    repeater: Repeater,
    /// Test window instance.
    window: Option<Window>,
    /// Identifier for the repeat stream.
    id: String,
    /// Key chord to drive repeats.
    chord: mac_keycode::Chord,
    /// Title assigned to the helper window.
    title: String,
    /// Whether the app has started driving repeats.
    started: bool,
    /// Start time for the run.
    start: Option<Instant>,
    /// Total duration to run the repeat.
    timeout: Duration,
    /// Whether the application requested shutdown.
    finished: bool,
}

impl ApplicationHandler for RelayApp {
    fn resumed(&mut self, elwt: &ActiveEventLoop) {
        if self.window.is_none() {
            use winit::dpi::{LogicalPosition, LogicalSize};
            let attrs = Window::default_attributes()
                .with_title(self.title.clone())
                .with_visible(true)
                .with_inner_size(LogicalSize::new(RELAY_WINDOW_WIDTH, RELAY_WINDOW_HEIGHT));
            let window = elwt.create_window(attrs).expect("create window");
            if let Some(mtm) = objc2_foundation::MainThreadMarker::new() {
                let app = objc2_app_kit::NSApplication::sharedApplication(mtm);
                unsafe { app.activate() };
            }
            if let Some(mtm) = objc2_foundation::MainThreadMarker::new() {
                use objc2_app_kit::NSScreen;
                if let Some(screen) = NSScreen::mainScreen(mtm) {
                    let vf = screen.visibleFrame();
                    let x =
                        (vf.origin.x + vf.size.width - RELAY_WINDOW_WIDTH - RELAY_WINDOW_MARGIN)
                            .max(0.0);
                    let y = (vf.origin.y + RELAY_WINDOW_MARGIN).max(0.0);
                    window.set_outer_position(LogicalPosition::new(x, y));
                }
            }
            self.window = Some(window);
        }
    }

    fn window_event(&mut self, elwt: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        if let WindowEvent::CloseRequested = event {
            self.repeater.stop_sync(&self.id);
            self.request_finish(elwt);
        }
    }

    fn about_to_wait(&mut self, elwt: &ActiveEventLoop) {
        if !self.started {
            self.started = true;
            self.repeater.start_relay_repeat(
                self.id.clone(),
                self.chord.clone(),
                Some(hotki_engine::RepeatSpec::default()),
            );
            self.start = Some(Instant::now());
        }

        if let Some(start) = self.start {
            if start.elapsed() >= self.timeout {
                self.repeater.stop_sync(&self.id);
                self.request_finish(elwt);
            }
            elwt.set_control_flow(ControlFlow::WaitUntil(start + self.timeout));
        } else {
            elwt.set_control_flow(ControlFlow::Wait);
        }
    }
}

impl RelayApp {
    /// Whether the application has requested shutdown.
    fn should_finish(&self) -> bool {
        self.finished
    }

    /// Next timeout used when pumping the shared event loop.
    fn next_wakeup_timeout(&self) -> Duration {
        if let Some(start) = self.start {
            let deadline = start + self.timeout;
            let now = Instant::now();
            if deadline <= now {
                Duration::from_millis(0)
            } else {
                (deadline - now).min(Duration::from_millis(16))
            }
        } else {
            Duration::from_millis(16)
        }
    }

    /// Request shutdown without calling `exit` so the loop stays reusable.
    fn request_finish(&mut self, elwt: &ActiveEventLoop) {
        if self.finished {
            return;
        }
        if let Some(window) = self.window.take() {
            window.set_visible(false);
        }
        self.finished = true;
        elwt.set_control_flow(ControlFlow::WaitUntil(Instant::now()));
    }
}

/// Pump the shared relay application until it signals shutdown.
fn run_relay_loop(handle: &EventLoopHandle, app: &mut RelayApp) -> Result<()> {
    loop {
        let timeout = app.next_wakeup_timeout();
        let status = handle.pump_app(Some(timeout), app);
        if app.should_finish() {
            break;
        }
        if let PumpStatus::Exit(code) = status {
            if code != 0 {
                return Err(Error::InvalidState(format!(
                    "relay repeat event loop exited with status {}",
                    code
                )));
            }
            break;
        }
    }
    Ok(())
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
