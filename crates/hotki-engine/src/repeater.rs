//! Unified repeat system for shell commands and key relays.
//!
//! Manages the execution and repetition of actions triggered by hotkeys,
//! including both shell command execution and key relay forwarding.
//! Provides configurable timing and cancellation support.

use std::{
    cmp,
    collections::HashMap,
    env, io,
    os::unix::process::CommandExt,
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU32, Ordering},
    },
};

use config::NotifyKind;
use hotki_protocol::FocusSnapshot;
use mac_keycode::Chord;
use parking_lot::Mutex;
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    process::Command,
    task::JoinHandle,
    time::Duration,
};
use tokio_util::sync::CancellationToken;
use tracing::trace;

use crate::{notification::NotificationDispatcher, relay::RelayHandler, ticker::Ticker};

#[derive(Debug, Clone)]
struct ShellNotification {
    kind: NotifyKind,
    title: String,
    text: String,
}

fn shell_failure(notify: ShellNotify, text: String) -> Option<ShellNotification> {
    if matches!(notify, ShellNotify::Silent) {
        return None;
    }
    Some(ShellNotification {
        kind: NotifyKind::Warn,
        title: "Shell command".to_string(),
        text,
    })
}

#[derive(Debug, Clone, Copy)]
enum ShellNotify {
    Configured {
        ok_notify: NotifyKind,
        err_notify: NotifyKind,
    },
    Silent,
}

/// Maximum combined stdout and stderr retained for a shell notification.
const SHELL_OUTPUT_LIMIT_BYTES: usize = 64 * 1024;
const SHELL_STREAM_LIMIT_BYTES: usize = SHELL_OUTPUT_LIMIT_BYTES / 2;

struct CapturedStream {
    bytes: Vec<u8>,
    truncated: bool,
}

async fn read_bounded(mut reader: impl AsyncRead + Unpin) -> io::Result<CapturedStream> {
    let mut bytes = Vec::with_capacity(SHELL_STREAM_LIMIT_BYTES);
    let mut truncated = false;
    let mut chunk = [0_u8; 4096];
    loop {
        let read = reader.read(&mut chunk).await?;
        if read == 0 {
            break;
        }
        let remaining = SHELL_STREAM_LIMIT_BYTES.saturating_sub(bytes.len());
        let retained = remaining.min(read);
        bytes.extend_from_slice(&chunk[..retained]);
        truncated |= retained < read;
    }
    Ok(CapturedStream { bytes, truncated })
}

fn trim_shell_output(stdout: CapturedStream, stderr: CapturedStream) -> String {
    let mut combined = String::new();
    let out = String::from_utf8_lossy(&stdout.bytes);
    let err = String::from_utf8_lossy(&stderr.bytes);
    if !out.is_empty() {
        combined.push_str(&out);
    }
    if !err.is_empty() {
        if !combined.is_empty() && !combined.ends_with('\n') {
            combined.push('\n');
        }
        combined.push_str(&err);
    }

    let lines: Vec<&str> = combined.lines().collect();
    let first_nonblank = lines.iter().position(|line| !line.trim().is_empty());
    let last_nonblank = lines.iter().rposition(|line| !line.trim().is_empty());
    let mut output = match (first_nonblank, last_nonblank) {
        (Some(start), Some(end)) if start <= end => lines[start..=end].join("\n"),
        _ => String::new(),
    };
    if stdout.truncated || stderr.truncated {
        if !output.is_empty() {
            output.push('\n');
        }
        output.push_str("[output truncated]");
    }
    output
}

fn kill_process_group(pid: u32) {
    let Ok(pid) = i32::try_from(pid) else {
        tracing::warn!(pid, "shell child pid exceeded process identifier range");
        return;
    };
    // SAFETY: the child was spawned as the leader of its own process group, and
    // a negative PID targets that group without borrowing any Rust memory.
    let result = unsafe { libc::kill(-pid, libc::SIGKILL) };
    if result != 0 && io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH) {
        tracing::warn!(pid, error = %io::Error::last_os_error(), "failed to kill shell process group");
    }
}

async fn collect_stream(task: Option<JoinHandle<io::Result<CapturedStream>>>) -> CapturedStream {
    match task {
        Some(task) => match task.await {
            Ok(Ok(stream)) => stream,
            Ok(Err(err)) => {
                tracing::warn!(error = %err, "failed to read shell output");
                CapturedStream {
                    bytes: Vec::new(),
                    truncated: false,
                }
            }
            Err(err) => {
                tracing::warn!(error = %err, "shell output reader task failed");
                CapturedStream {
                    bytes: Vec::new(),
                    truncated: false,
                }
            }
        },
        None => CapturedStream {
            bytes: Vec::new(),
            truncated: false,
        },
    }
}

/// Run a command in an owned process group with bounded output capture.
async fn run_shell(
    command: &str,
    notify: ShellNotify,
    state: &ShellRunState,
) -> Option<ShellNotification> {
    tracing::info!("Executing shell command: {}", command);
    let shell_path: String = env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string());
    let capture_output = !matches!(notify, ShellNotify::Silent)
        && !matches!(
            notify,
            ShellNotify::Configured {
                ok_notify: NotifyKind::Ignore,
                err_notify: NotifyKind::Ignore,
            }
        );
    let mut command_builder = Command::new(&shell_path);
    command_builder.arg("-lc").arg(command).kill_on_drop(true);
    command_builder
        .as_std_mut()
        .process_group(0)
        .stdout(if capture_output {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stderr(if capture_output {
            Stdio::piped()
        } else {
            Stdio::null()
        });

    let mut child = match command_builder.spawn() {
        Ok(child) => child,
        Err(err) => {
            return shell_failure(notify, format!("Failed to execute: {err}"));
        }
    };
    let Some(pid) = child.id() else {
        return shell_failure(
            notify,
            "Failed to obtain child process identifier".to_string(),
        );
    };
    state.pid.store(pid, Ordering::SeqCst);
    let stdout_task = child
        .stdout
        .take()
        .map(|stdout| tokio::spawn(read_bounded(stdout)));
    let stderr_task = child
        .stderr
        .take()
        .map(|stderr| tokio::spawn(read_bounded(stderr)));

    let status = tokio::select! {
        status = child.wait() => {
            kill_process_group(pid);
            Some(status)
        },
        () = state.cancel.cancelled() => {
            kill_process_group(pid);
            let _ = child.wait().await;
            None
        }
    };
    state.pid.store(0, Ordering::SeqCst);
    let stdout = collect_stream(stdout_task).await;
    let stderr = collect_stream(stderr_task).await;
    if state.cancel.is_cancelled() {
        return None;
    }
    let status = status?;
    let status = match status {
        Ok(status) => status,
        Err(err) => {
            return shell_failure(notify, format!("Failed to wait for command: {err}"));
        }
    };
    let (ok_notify, err_notify) = match notify {
        ShellNotify::Configured {
            ok_notify,
            err_notify,
        } => (ok_notify, err_notify),
        ShellNotify::Silent => return None,
    };
    let kind = if status.success() {
        ok_notify
    } else {
        err_notify
    };
    match kind {
        NotifyKind::Ignore => None,
        kind => Some(ShellNotification {
            kind,
            title: "Shell command".to_string(),
            text: trim_shell_output(stdout, stderr),
        }),
    }
}

/// Clamp bounds and defaults for repeat timings (applies to relay + shell repeaters)
const REPEAT_MIN_INITIAL_DELAY_MS: u64 = 100;
const REPEAT_MAX_INITIAL_DELAY_MS: u64 = 1000;
const REPEAT_MIN_INTERVAL_MS: u64 = 100;
const REPEAT_MAX_INTERVAL_MS: u64 = 2000;
const REPEAT_DEFAULT_MIN_INTERVAL_MS: u64 = 150;

/// System default timings for key repeat
const SYS_INITIAL_DELAY_MS: u64 = 250; // Initial delay before first repeat
const SYS_INTERVAL_MS: u64 = 33; // ~30 repeats per second

/// What to execute (first run is immediate; repeats are driven by the repeater)
#[derive(Clone)]
pub enum ExecSpec {
    Shell {
        command: String,
        ok_notify: NotifyKind,
        err_notify: NotifyKind,
    },
    Relay {
        chord: Chord,
    },
}

/// Optional repeat configuration overrides
#[derive(Clone, Copy, Debug, Default)]
pub struct RepeatSpec {
    /// Optional initial delay before the first repeat (milliseconds).
    pub initial_delay_ms: Option<u64>,
    /// Optional interval between repeats (milliseconds).
    pub interval_ms: Option<u64>,
}

/// Per-id state to serialize shell command execution and coalesce repeats.
///
/// Semantics:
/// - The first shell run for an id starts immediately and sets `running = true`.
/// - While `running` is true, any scheduled repeat ticks are skipped (coalesced).
/// - When the first run (or any repeat run) completes, `running` is set back to false,
///   allowing the next tick to execute. This guarantees that at most one shell
///   command is in-flight per id, and that the first blocking run effectively
///   defers the start of repeating until it finishes.
struct ShellRunState {
    /// Async mutex to serialize execution across initial run and repeats.
    gate: tokio::sync::Mutex<()>,
    /// Fast flag used to skip scheduling a repeat while a run is in-flight.
    running: AtomicBool,
    /// Cancels the running process group and suppresses its notification.
    cancel: CancellationToken,
    /// Current owned task; at most one shell command runs per binding id.
    task: Mutex<Option<JoinHandle<()>>>,
    /// Process-group leader while a command is running.
    pid: AtomicU32,
    /// Whether key release cancels the current child process.
    cancel_on_release: bool,
}

impl ShellRunState {
    fn set_task(&self, task: JoinHandle<()>) {
        if let Some(previous) = self.task.lock().replace(task) {
            debug_assert!(previous.is_finished());
        }
    }

    async fn stop(&self) {
        self.cancel.cancel();
        let task = self.task.lock().take();
        if let Some(task) = task {
            let _ = task.await;
        }
    }

    fn abort(&self) {
        self.cancel.cancel();
        let _ = self.task.lock().take();
        let pid = self.pid.swap(0, Ordering::SeqCst);
        if pid != 0 {
            kill_process_group(pid);
        }
    }
}

fn spawn_shell_run(
    notifier: NotificationDispatcher,
    on_shell_repeat: Arc<Mutex<Option<OnShellRepeat>>>,
    state: Arc<ShellRunState>,
    command: String,
    notify: ShellNotify,
    repeat_id: Option<String>,
) {
    let task_state = state.clone();
    let task = tokio::spawn(async move {
        let _guard = state.gate.lock().await;
        if let Some(notification) = run_shell(&command, notify, state.as_ref()).await
            && let Err(err) =
                notifier.send_notification(notification.kind, notification.title, notification.text)
        {
            tracing::warn!("Failed to deliver shell notification: {}", err);
        }

        if !state.cancel.is_cancelled()
            && let Some(id) = repeat_id
            && let Some(cb) = on_shell_repeat.lock().as_ref()
        {
            cb(&id);
            trace!("repeater_shell_run_done" = %id);
        }

        state.running.store(false, Ordering::SeqCst);
    });
    task_state.set_task(task);
}

/// Callback type for observing relay repeat ticks (used by tests/tools).
pub type OnRelayRepeat = Arc<dyn Fn(&str) + Send + Sync>;

/// Callback type for observing shell repeat ticks (used by tests/tools).
pub type OnShellRepeat = Arc<dyn Fn(&str) + Send + Sync>;

/// Unified repeater that runs first-run immediately and then repeats while held
#[derive(Clone)]
pub struct Repeater {
    sys_initial: Duration,
    sys_interval: Duration,
    /// Focus snapshot providing the current target PID.
    focus_ctx: Arc<Mutex<Option<FocusSnapshot>>>,
    relay: RelayHandler,
    notifier: NotificationDispatcher,
    ticker: Ticker,
    /// Optional callback for relay repeat instrumentation.
    on_relay_repeat: Arc<Mutex<Option<OnRelayRepeat>>>,
    /// Optional callback for shell repeat instrumentation.
    on_shell_repeat: Arc<Mutex<Option<OnShellRepeat>>>,
    /// Per-id state for shell execution serialization.
    shell_states: Arc<Mutex<HashMap<String, Arc<ShellRunState>>>>,
}

impl Repeater {
    /// Create a repeater backed by a focus context (world-derived).
    pub fn new_with_ctx(
        focus_ctx: Arc<Mutex<Option<FocusSnapshot>>>,
        relay: RelayHandler,
        notifier: NotificationDispatcher,
    ) -> Self {
        let sys_initial = Duration::from_millis(SYS_INITIAL_DELAY_MS);
        let sys_interval = Duration::from_millis(SYS_INTERVAL_MS);
        Self {
            sys_initial,
            sys_interval,
            focus_ctx,
            relay,
            notifier,
            ticker: Ticker::default(),
            on_relay_repeat: Arc::new(Mutex::new(None)),
            on_shell_repeat: Arc::new(Mutex::new(None)),
            shell_states: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Get the current focused PID, or -1 if unknown.
    fn current_pid(&self) -> i32 {
        self.focus_ctx
            .lock()
            .as_ref()
            .map(|focus| focus.pid)
            .unwrap_or(-1)
    }

    /// Get or create the per-id shell run state.
    fn shell_state(&self, id: &str, cancel_on_release: bool) -> Arc<ShellRunState> {
        let mut map = self.shell_states.lock();
        if let Some(s) = map.get(id) {
            return s.clone();
        }
        let state = Arc::new(ShellRunState {
            gate: tokio::sync::Mutex::new(()),
            running: AtomicBool::new(false),
            cancel: CancellationToken::new(),
            task: Mutex::new(None),
            pid: AtomicU32::new(0),
            cancel_on_release,
        });
        map.insert(id.to_string(), state.clone());
        state
    }

    pub(crate) fn effective_timings(&self, spec: Option<RepeatSpec>) -> (Duration, Duration) {
        let sys_initial_ms = self.sys_initial.as_millis() as u64;
        let sys_interval_ms = self.sys_interval.as_millis() as u64;
        let default_interval = cmp::max(sys_interval_ms, REPEAT_DEFAULT_MIN_INTERVAL_MS);

        let (i_ms, t_ms) = if let Some(spec) = spec {
            (
                spec.initial_delay_ms
                    .unwrap_or(sys_initial_ms)
                    .clamp(REPEAT_MIN_INITIAL_DELAY_MS, REPEAT_MAX_INITIAL_DELAY_MS),
                spec.interval_ms
                    .unwrap_or(default_interval)
                    .clamp(REPEAT_MIN_INTERVAL_MS, REPEAT_MAX_INTERVAL_MS),
            )
        } else {
            (
                sys_initial_ms.clamp(REPEAT_MIN_INITIAL_DELAY_MS, REPEAT_MAX_INITIAL_DELAY_MS),
                default_interval.clamp(REPEAT_MIN_INTERVAL_MS, REPEAT_MAX_INTERVAL_MS),
            )
        };
        (Duration::from_millis(i_ms), Duration::from_millis(t_ms))
    }

    fn spawn_repeat_loop(
        &self,
        id: String,
        repeat: Option<RepeatSpec>,
        tick: impl FnMut() + Send + 'static,
    ) {
        let (initial_delay, interval) = self.effective_timings(repeat);
        self.ticker.start(id, initial_delay, interval, tick);
    }

    fn spawn_shell_run(
        &self,
        state: Arc<ShellRunState>,
        command: String,
        notify: ShellNotify,
        repeat_id: Option<String>,
    ) {
        spawn_shell_run(
            self.notifier.clone(),
            self.on_shell_repeat.clone(),
            state,
            command,
            notify,
            repeat_id,
        );
    }

    /// Optional: install a relay repeat callback for instrumentation/testing.
    #[cfg(test)]
    pub(crate) fn set_on_relay_repeat(&self, cb: OnRelayRepeat) {
        *self.on_relay_repeat.lock() = Some(cb);
    }

    /// Notify relay repeat instrumentation for an externally driven repeat loop.
    pub(crate) fn note_relay_repeat(&self, id: &str) {
        if let Some(cb) = self.on_relay_repeat.lock().as_ref() {
            cb(id);
        }
    }

    /// Optional: install a shell repeat callback for instrumentation/testing.
    #[cfg(test)]
    fn set_on_shell_repeat(&self, cb: OnShellRepeat) {
        *self.on_shell_repeat.lock() = Some(cb);
    }

    /// Start execution for a binding id.
    ///
    /// Runs the first action immediately and schedules repeats if provided.
    pub fn start(&self, id: String, exec: ExecSpec, repeat: Option<RepeatSpec>) {
        self.ticker.abort(&id);
        if let Some(state) = self.shell_states.lock().remove(&id) {
            state.abort();
        }

        self.start_initial_exec(&id, &exec, repeat.is_some());

        if repeat.is_some() {
            self.spawn_exec_repeater(id, exec, repeat);
        }
    }

    fn start_initial_exec(&self, id: &str, exec: &ExecSpec, cancel_on_release: bool) {
        match exec {
            ExecSpec::Shell {
                command,
                ok_notify,
                err_notify,
            } => {
                let state = self.shell_state(id, cancel_on_release);
                state.running.store(true, Ordering::SeqCst);
                self.spawn_shell_run(
                    state,
                    command.clone(),
                    ShellNotify::Configured {
                        ok_notify: *ok_notify,
                        err_notify: *err_notify,
                    },
                    None,
                );
            }
            ExecSpec::Relay { chord } => {
                let pid = self.current_pid();
                self.relay
                    .start_relay(id.to_string(), chord.clone(), pid, false);
            }
        }
    }

    fn spawn_exec_repeater(&self, id: String, exec: ExecSpec, repeat: Option<RepeatSpec>) {
        match exec {
            ExecSpec::Shell { command, .. } => self.spawn_shell_repeat_loop(id, command, repeat),
            ExecSpec::Relay { chord } => {
                let initial_pid = self.current_pid();
                self.spawn_relay_repeat_loop(id, chord, initial_pid, repeat);
            }
        }
    }

    fn spawn_shell_repeat_loop(&self, id: String, command: String, repeat: Option<RepeatSpec>) {
        let id_for_log = id.clone();
        let state = self.shell_state(&id, true);
        let notifier = self.notifier.clone();
        let on_shell_repeat = self.on_shell_repeat.clone();
        self.spawn_repeat_loop(id, repeat, move || {
            if state.running.swap(true, Ordering::SeqCst) {
                trace!("repeater_shell_tick_skip_running" = %id_for_log);
                return;
            }
            spawn_shell_run(
                notifier.clone(),
                on_shell_repeat.clone(),
                state.clone(),
                command.clone(),
                ShellNotify::Silent,
                Some(id_for_log.clone()),
            );
        });
    }

    fn spawn_relay_repeat_loop(
        &self,
        id: String,
        chord: Chord,
        initial_pid: i32,
        repeat: Option<RepeatSpec>,
    ) {
        let relay = self.relay.clone();
        let focus_ctx = self.focus_ctx.clone();
        let id_for_log = id.clone();
        let ch = chord;

        let mut last_pid = initial_pid;
        let on_relay_repeat = self.on_relay_repeat.clone();
        let running_flag = Arc::new(AtomicBool::new(false));
        let repeat_running = running_flag;
        self.spawn_repeat_loop(id, repeat, move || {
            let pid = focus_ctx
                .lock()
                .as_ref()
                .map(|focus| focus.pid)
                .unwrap_or(-1);
            if pid != -1 && pid != last_pid {
                relay.stop_relay(&id_for_log, last_pid);
                relay.start_relay(id_for_log.clone(), ch.clone(), pid, false);
                last_pid = pid;
                return;
            }
            if pid != -1 {
                if repeat_running.swap(true, Ordering::SeqCst) {
                    trace!("repeater_relay_tick_skip_running" = %id_for_log);
                    return;
                }
                if relay.repeat_relay(&id_for_log)
                    && let Some(cb) = on_relay_repeat.lock().as_ref()
                {
                    cb(&id_for_log);
                }
                repeat_running.store(false, Ordering::SeqCst);
            }
        });
    }

    /// Mark OS-level repeat seen for id; stop software ticker repeats
    pub async fn note_os_repeat(&self, id: &str) {
        self.ticker.stop(id).await;
    }

    /// Stop any software repeats for `id`.
    pub async fn stop(&self, id: &str) {
        self.ticker.stop(id).await;
        let state = self.shell_states.lock().get(id).cloned();
        if let Some(state) = state
            && state.cancel_on_release
        {
            self.shell_states.lock().remove(id);
            state.stop().await;
        }
    }

    /// Returns true if the ticker is currently running for `id`.
    pub fn is_ticking(&self, id: &str) -> bool {
        self.ticker.is_active(id)
    }

    /// Stop all tickers asynchronously and wait for completion.
    pub async fn clear_async(&self) {
        self.ticker.clear_async().await;
        let states: Vec<_> = self
            .shell_states
            .lock()
            .drain()
            .map(|(_, state)| state)
            .collect();
        for state in &states {
            state.cancel.cancel();
        }
        for state in states {
            state.stop().await;
        }
    }

    /// Abort all repeat tasks during synchronous owner teardown.
    pub(crate) fn abort_all(&self) {
        self.ticker.abort_all();
        for state in self.shell_states.lock().drain().map(|(_, state)| state) {
            state.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        io::Cursor,
        sync::atomic::{AtomicUsize, Ordering as AtomicOrdering},
        time::{Duration as StdDuration, Instant},
    };

    use tokio::{
        sync::mpsc,
        time::{Duration, advance, sleep},
    };

    use super::*;

    #[tokio::test]
    async fn shell_output_capture_is_bounded_and_marks_truncation() {
        let input = vec![b'x'; SHELL_STREAM_LIMIT_BYTES + 4096];
        let capture = read_bounded(Cursor::new(input))
            .await
            .expect("read capture");

        assert_eq!(capture.bytes.len(), SHELL_STREAM_LIMIT_BYTES);
        assert!(capture.truncated);
        let output = trim_shell_output(
            capture,
            CapturedStream {
                bytes: Vec::new(),
                truncated: false,
            },
        );
        assert!(output.ends_with("[output truncated]"));
    }

    #[tokio::test]
    async fn stopping_shell_action_terminates_owned_process_group() {
        let focus_ctx = Arc::new(Mutex::new(None::<FocusSnapshot>));
        let relay = crate::RelayHandler::new_with_enabled(false);
        let (tx, _rx) = mpsc::channel(16);
        let notifier = crate::notification::NotificationDispatcher::new(tx);
        let repeater = Repeater::new_with_ctx(focus_ctx, relay, notifier);

        repeater.start(
            "owned-shell".to_string(),
            ExecSpec::Shell {
                command: "sleep 30".to_string(),
                ok_notify: NotifyKind::Ignore,
                err_notify: NotifyKind::Ignore,
            },
            Some(RepeatSpec::default()),
        );
        let state = repeater.shell_state("owned-shell", true);
        let pid = loop {
            let pid = state.pid.load(Ordering::SeqCst);
            if pid != 0 {
                break pid;
            }
            tokio::task::yield_now().await;
        };

        // SAFETY: signal zero only probes the process identifier and does not
        // affect the child or access Rust memory.
        assert_eq!(unsafe { libc::kill(-(pid as i32), 0) }, 0);
        repeater.stop("owned-shell").await;
        // SAFETY: this repeats the non-mutating process-existence probe above.
        assert_eq!(unsafe { libc::kill(-(pid as i32), 0) }, -1);
        assert_eq!(io::Error::last_os_error().raw_os_error(), Some(libc::ESRCH));
    }

    #[tokio::test]
    async fn clearing_repeater_terminates_one_shot_process_group() {
        let focus_ctx = Arc::new(Mutex::new(None::<FocusSnapshot>));
        let relay = crate::RelayHandler::new_with_enabled(false);
        let (tx, _rx) = mpsc::channel(16);
        let notifier = crate::notification::NotificationDispatcher::new(tx);
        let repeater = Repeater::new_with_ctx(focus_ctx, relay, notifier);

        repeater.start(
            "one-shot-shell".to_string(),
            ExecSpec::Shell {
                command: "sleep 30".to_string(),
                ok_notify: NotifyKind::Ignore,
                err_notify: NotifyKind::Ignore,
            },
            None,
        );
        let state = repeater.shell_state("one-shot-shell", false);
        let pid = loop {
            let pid = state.pid.load(Ordering::SeqCst);
            if pid != 0 {
                break pid;
            }
            tokio::task::yield_now().await;
        };

        repeater.clear_async().await;
        // SAFETY: signal zero only probes the process group and does not affect it.
        assert_eq!(unsafe { libc::kill(-(pid as i32), 0) }, -1);
        assert_eq!(io::Error::last_os_error().raw_os_error(), Some(libc::ESRCH));
    }

    #[tokio::test]
    async fn configured_shell_action_delivers_captured_output() {
        let focus_ctx = Arc::new(Mutex::new(None::<FocusSnapshot>));
        let relay = crate::RelayHandler::new_with_enabled(false);
        let (tx, mut rx) = mpsc::channel(16);
        let notifier = crate::notification::NotificationDispatcher::new(tx);
        let repeater = Repeater::new_with_ctx(focus_ctx, relay, notifier);

        repeater.start(
            "captured-shell".to_string(),
            ExecSpec::Shell {
                command: "echo notify".to_string(),
                ok_notify: NotifyKind::Info,
                err_notify: NotifyKind::Warn,
            },
            None,
        );

        repeater.stop("captured-shell").await;
        let message = rx.recv().await.expect("shell notification");
        assert!(matches!(
            message,
            hotki_protocol::MsgToUI::Notify {
                kind: NotifyKind::Info,
                text,
                ..
            } if text == "notify"
        ));
    }

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn shell_first_run_coalesces_then_unblocks_repeats() {
        let focus_ctx = Arc::new(Mutex::new(None::<FocusSnapshot>));
        let relay = crate::RelayHandler::new_with_enabled(false);
        let (tx, _rx) = mpsc::channel(16);
        let notifier = crate::notification::NotificationDispatcher::new(tx);
        let repeater = Repeater::new_with_ctx(focus_ctx, relay, notifier);

        let shell_count = Arc::new(AtomicUsize::new(0));
        let shell_count2 = shell_count.clone();
        repeater.set_on_shell_repeat(Arc::new(move |_id| {
            shell_count2.fetch_add(1, AtomicOrdering::SeqCst);
        }));

        // Use a fast command, but hold the per-id gate so the initial run overlaps
        // the first repeat tick without relying on real time.
        let cmd = "true".to_string();

        repeater.start(
            "shell-coalesce".to_string(),
            ExecSpec::Shell {
                command: cmd,
                ok_notify: config::NotifyKind::Ignore,
                err_notify: config::NotifyKind::Ignore,
            },
            Some(RepeatSpec {
                initial_delay_ms: Some(100),
                interval_ms: Some(100),
            }),
        );

        // After ~150ms the first repeat tick would have fired; ensure it was coalesced
        let state = repeater.shell_state("shell-coalesce", true);
        let gate_guard = state.gate.lock().await;
        tokio::task::yield_now().await; // let ticker + initial run tasks start and register timers
        advance(Duration::from_millis(150)).await;
        sleep(Duration::from_millis(0)).await; // yield so ticker task observes time advance
        assert_eq!(
            shell_count.load(AtomicOrdering::SeqCst),
            0,
            "No shell repeats during the first blocking run",
        );

        drop(gate_guard);
        tokio::task::yield_now().await; // allow initial run to start
        let real_start = Instant::now();
        while state.running.load(AtomicOrdering::SeqCst)
            && real_start.elapsed() < StdDuration::from_secs(1)
        {
            tokio::task::yield_now().await;
        }
        assert!(
            !state.running.load(AtomicOrdering::SeqCst),
            "initial shell run should complete"
        );

        // Advance far enough for the next repeat ticks to execute after initial completion.
        advance(Duration::from_millis(250)).await;
        let real_start_repeat = Instant::now();
        while shell_count.load(AtomicOrdering::SeqCst) == 0
            && real_start_repeat.elapsed() < StdDuration::from_secs(1)
        {
            tokio::task::yield_now().await;
        }
        let repeats = shell_count.load(AtomicOrdering::SeqCst);
        repeater.stop("shell-coalesce").await;
        assert!(
            repeats >= 1,
            "At least one shell repeat should run after the first run completes",
        );
    }
}
