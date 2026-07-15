//! Repeat system for owned processes.
//!
//! Manages the execution and repetition of actions triggered by hotkeys,
//! including both direct and shell-backed process execution. Provides
//! configurable timing and cancellation support.

use std::{
    cmp,
    collections::HashMap,
    env, io,
    os::unix::process::CommandExt,
    path::{Component, Path, PathBuf},
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU32, Ordering},
    },
};

use config::NotifyKind;
use parking_lot::Mutex;
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    process::Command,
    task::JoinHandle,
    time::Duration,
};
use tokio_util::sync::CancellationToken;
use tracing::trace;

use crate::{notification::NotificationDispatcher, ticker::Ticker};

#[derive(Debug, Clone)]
struct ProcessNotification {
    kind: NotifyKind,
    title: String,
    text: String,
}

fn process_failure(
    spec: &ProcessSpec,
    notify: ProcessNotify,
    text: String,
) -> Option<ProcessNotification> {
    let kind = match notify {
        ProcessNotify::Configured { err_notify, .. } => err_notify,
        ProcessNotify::Silent => return None,
    };
    match kind {
        NotifyKind::Ignore => None,
        kind => Some(ProcessNotification {
            kind,
            title: spec.title.to_string(),
            text,
        }),
    }
}

#[derive(Debug, Clone, Copy)]
enum ProcessNotify {
    Configured {
        ok_notify: NotifyKind,
        err_notify: NotifyKind,
    },
    Silent,
}

/// Maximum combined stdout and stderr retained for a process notification.
const PROCESS_OUTPUT_LIMIT_BYTES: usize = 64 * 1024;
const PROCESS_STREAM_LIMIT_BYTES: usize = PROCESS_OUTPUT_LIMIT_BYTES / 2;

struct CapturedStream {
    bytes: Vec<u8>,
    truncated: bool,
}

async fn read_bounded(mut reader: impl AsyncRead + Unpin) -> io::Result<CapturedStream> {
    let mut bytes = Vec::with_capacity(PROCESS_STREAM_LIMIT_BYTES);
    let mut truncated = false;
    let mut chunk = [0_u8; 4096];
    loop {
        let read = reader.read(&mut chunk).await?;
        if read == 0 {
            break;
        }
        let remaining = PROCESS_STREAM_LIMIT_BYTES.saturating_sub(bytes.len());
        let retained = remaining.min(read);
        bytes.extend_from_slice(&chunk[..retained]);
        truncated |= retained < read;
    }
    Ok(CapturedStream { bytes, truncated })
}

fn trim_process_output(stdout: CapturedStream, stderr: CapturedStream) -> String {
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
        tracing::warn!(pid, "process child pid exceeded process identifier range");
        return;
    };
    // SAFETY: the child was spawned as the leader of its own process group, and
    // a negative PID targets that group without borrowing any Rust memory.
    let result = unsafe { libc::kill(-pid, libc::SIGKILL) };
    if result != 0 && io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH) {
        tracing::warn!(pid, error = %io::Error::last_os_error(), "failed to kill process group");
    }
}

async fn collect_stream(task: Option<JoinHandle<io::Result<CapturedStream>>>) -> CapturedStream {
    match task {
        Some(task) => match task.await {
            Ok(Ok(stream)) => stream,
            Ok(Err(err)) => {
                tracing::warn!(error = %err, "failed to read process output");
                CapturedStream {
                    bytes: Vec::new(),
                    truncated: false,
                }
            }
            Err(err) => {
                tracing::warn!(error = %err, "process output reader task failed");
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

/// Return the inherited PATH in a diagnostic-safe display form.
fn effective_path() -> String {
    env::var_os("PATH")
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_else(|| "<unset>".to_string())
}

fn is_bare_program(program: &str) -> bool {
    let mut components = Path::new(program).components();
    !program.is_empty()
        && !program.contains('/')
        && matches!(components.next(), Some(Component::Normal(_)))
        && components.next().is_none()
}

/// Run one process in an owned process group with bounded output capture.
async fn run_process(
    spec: &ProcessSpec,
    notify: ProcessNotify,
    state: &ProcessRunState,
) -> Option<ProcessNotification> {
    tracing::info!(
        program = %spec.program,
        args = ?spec.args,
        cwd = ?spec.cwd,
        "Executing process",
    );
    let capture_output = !matches!(notify, ProcessNotify::Silent)
        && !matches!(
            notify,
            ProcessNotify::Configured {
                ok_notify: NotifyKind::Ignore,
                err_notify: NotifyKind::Ignore,
            }
        );
    let mut command_builder = Command::new(&spec.program);
    command_builder.args(&spec.args).kill_on_drop(true);
    if let Some(cwd) = &spec.cwd {
        command_builder.current_dir(cwd);
    }
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
            let cwd_is_directory = spec.cwd.as_deref().is_none_or(Path::is_dir);
            let path = if err.kind() == io::ErrorKind::NotFound
                && is_bare_program(&spec.program)
                && cwd_is_directory
            {
                format!("; effective PATH={}", effective_path())
            } else {
                String::new()
            };
            return process_failure(
                spec,
                notify,
                format!("Failed to execute {}: {err}{path}", spec.program),
            );
        }
    };
    let Some(pid) = child.id() else {
        return process_failure(
            spec,
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
            return process_failure(spec, notify, format!("Failed to wait for process: {err}"));
        }
    };
    let (ok_notify, err_notify) = match notify {
        ProcessNotify::Configured {
            ok_notify,
            err_notify,
        } => (ok_notify, err_notify),
        ProcessNotify::Silent => return None,
    };
    let kind = if status.success() {
        ok_notify
    } else {
        err_notify
    };
    match kind {
        NotifyKind::Ignore => None,
        kind => Some(ProcessNotification {
            kind,
            title: spec.title.to_string(),
            text: trim_process_output(stdout, stderr),
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

/// Direct process specification shared by shell and `exec` actions.
#[derive(Clone)]
pub(crate) struct ProcessSpec {
    /// Program path or bare program name.
    pub(crate) program: String,
    /// Literal process arguments.
    pub(crate) args: Vec<String>,
    /// Optional process working directory.
    pub(crate) cwd: Option<PathBuf>,
    /// Notification type for successful exit.
    pub(crate) ok_notify: NotifyKind,
    /// Notification type for error exit.
    pub(crate) err_notify: NotifyKind,
    /// Notification title for this process class.
    pub(crate) title: &'static str,
}

impl ProcessSpec {
    /// Construct a process specification with an explicit notification title.
    pub(crate) fn new(
        program: impl Into<String>,
        args: Vec<String>,
        cwd: Option<PathBuf>,
        ok_notify: NotifyKind,
        err_notify: NotifyKind,
        title: &'static str,
    ) -> Self {
        Self {
            program: program.into(),
            args,
            cwd,
            ok_notify,
            err_notify,
            title,
        }
    }

    /// Construct the shell-language process using the inherited shell choice.
    pub(crate) fn shell(command: String, ok_notify: NotifyKind, err_notify: NotifyKind) -> Self {
        let shell = env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string());
        Self::new(
            shell,
            vec!["-lc".to_string(), command],
            None,
            ok_notify,
            err_notify,
            "Shell command",
        )
    }
}

/// Optional repeat configuration overrides
#[derive(Clone, Copy, Debug, Default)]
pub struct RepeatSpec {
    /// Optional initial delay before the first repeat (milliseconds).
    pub initial_delay_ms: Option<u64>,
    /// Optional interval between repeats (milliseconds).
    pub interval_ms: Option<u64>,
}

/// Per-id state to serialize process execution and coalesce repeats.
///
/// Semantics:
/// - The first process run for an id starts immediately and sets `running = true`.
/// - While `running` is true, any scheduled repeat ticks are skipped (coalesced).
/// - When the first run (or any repeat run) completes, `running` is set back to false,
///   allowing the next tick to execute. This guarantees that at most one process
///   is in-flight per id, and that the first blocking run effectively
///   defers the start of repeating until it finishes.
struct ProcessRunState {
    /// Async mutex to serialize execution across initial run and repeats.
    gate: tokio::sync::Mutex<()>,
    /// Fast flag used to skip scheduling a repeat while a run is in-flight.
    running: AtomicBool,
    /// Cancels the running process group and suppresses its notification.
    cancel: CancellationToken,
    /// Current owned task; at most one process runs per binding id.
    task: Mutex<Option<JoinHandle<()>>>,
    /// Process-group leader while a command is running.
    pid: AtomicU32,
    /// Whether key release cancels the current child process.
    cancel_on_release: bool,
}

impl ProcessRunState {
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

struct ProcessRunContext {
    notifier: NotificationDispatcher,
    on_process_repeat: Arc<Mutex<Option<OnProcessRepeat>>>,
    process_states: Arc<Mutex<HashMap<String, Arc<ProcessRunState>>>>,
}

impl ProcessRunContext {
    fn spawn(
        &self,
        state: Arc<ProcessRunState>,
        spec: ProcessSpec,
        notify: ProcessNotify,
        repeat_id: Option<String>,
        state_id: String,
    ) {
        let task_state = state.clone();
        let notifier = self.notifier.clone();
        let on_process_repeat = self.on_process_repeat.clone();
        let process_states = self.process_states.clone();
        let task = tokio::spawn(async move {
            let _guard = state.gate.lock().await;
            let notification = run_process(&spec, notify, state.as_ref()).await;
            state.task.lock().take();
            state.running.store(false, Ordering::SeqCst);

            if repeat_id.is_none() && !state.cancel_on_release {
                let mut states = process_states.lock();
                if states
                    .get(&state_id)
                    .is_some_and(|current| Arc::ptr_eq(current, &state))
                {
                    states.remove(&state_id);
                }
            }

            if let Some(notification) = notification
                && let Err(err) = notifier.send_notification(
                    notification.kind,
                    notification.title,
                    notification.text,
                )
            {
                tracing::warn!("Failed to deliver process notification: {}", err);
            }

            if !state.cancel.is_cancelled()
                && let Some(id) = repeat_id
                && let Some(cb) = on_process_repeat.lock().as_ref()
            {
                cb(&id);
                trace!("repeater_process_run_done" = %id);
            }
        });
        task_state.set_task(task);
    }
}

/// Callback type for observing relay repeat ticks (used by tests/tools).
pub type OnRelayRepeat = Arc<dyn Fn(&str) + Send + Sync>;

/// Callback type for observing process repeat ticks (used by tests/tools).
pub(crate) type OnProcessRepeat = Arc<dyn Fn(&str) + Send + Sync>;

/// Unified repeater that runs first-run immediately and then repeats while held
#[derive(Clone)]
pub struct Repeater {
    sys_initial: Duration,
    sys_interval: Duration,
    notifier: NotificationDispatcher,
    ticker: Ticker,
    /// Optional callback for relay repeat instrumentation.
    on_relay_repeat: Arc<Mutex<Option<OnRelayRepeat>>>,
    /// Optional callback for process repeat instrumentation.
    on_process_repeat: Arc<Mutex<Option<OnProcessRepeat>>>,
    /// Per-id state for process execution serialization.
    process_states: Arc<Mutex<HashMap<String, Arc<ProcessRunState>>>>,
}

impl Repeater {
    /// Create a process repeater.
    pub fn new(notifier: NotificationDispatcher) -> Self {
        let sys_initial = Duration::from_millis(SYS_INITIAL_DELAY_MS);
        let sys_interval = Duration::from_millis(SYS_INTERVAL_MS);
        Self {
            sys_initial,
            sys_interval,
            notifier,
            ticker: Ticker::default(),
            on_relay_repeat: Arc::new(Mutex::new(None)),
            on_process_repeat: Arc::new(Mutex::new(None)),
            process_states: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Get or create the per-id process run state.
    fn process_state(&self, id: &str, cancel_on_release: bool) -> Arc<ProcessRunState> {
        let mut map = self.process_states.lock();
        if let Some(s) = map.get(id) {
            return s.clone();
        }
        let state = Arc::new(ProcessRunState {
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

    fn spawn_process_run(
        &self,
        state: Arc<ProcessRunState>,
        spec: ProcessSpec,
        notify: ProcessNotify,
        repeat_id: Option<String>,
        state_id: String,
    ) {
        ProcessRunContext {
            notifier: self.notifier.clone(),
            on_process_repeat: self.on_process_repeat.clone(),
            process_states: self.process_states.clone(),
        }
        .spawn(state, spec, notify, repeat_id, state_id);
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

    /// Optional: install a process repeat callback for instrumentation/testing.
    #[cfg(test)]
    fn set_on_process_repeat(&self, cb: OnProcessRepeat) {
        *self.on_process_repeat.lock() = Some(cb);
    }

    /// Start execution for a binding id.
    ///
    /// Runs the first action immediately and schedules repeats if provided.
    pub fn start(&self, id: String, process: ProcessSpec, repeat: Option<RepeatSpec>) {
        self.ticker.abort(&id);
        if let Some(state) = self.process_states.lock().remove(&id) {
            state.abort();
        }

        self.start_initial_process(&id, &process, repeat.is_some());

        if repeat.is_some() {
            self.spawn_process_repeat_loop(id, process, repeat);
        }
    }

    fn start_initial_process(&self, id: &str, spec: &ProcessSpec, cancel_on_release: bool) {
        let state = self.process_state(id, cancel_on_release);
        state.running.store(true, Ordering::SeqCst);
        self.spawn_process_run(
            state,
            spec.clone(),
            ProcessNotify::Configured {
                ok_notify: spec.ok_notify,
                err_notify: spec.err_notify,
            },
            None,
            id.to_string(),
        );
    }

    fn spawn_process_repeat_loop(&self, id: String, spec: ProcessSpec, repeat: Option<RepeatSpec>) {
        let id_for_log = id.clone();
        let state = self.process_state(&id, true);
        let process_runner = ProcessRunContext {
            notifier: self.notifier.clone(),
            on_process_repeat: self.on_process_repeat.clone(),
            process_states: self.process_states.clone(),
        };
        self.spawn_repeat_loop(id, repeat, move || {
            if state.running.swap(true, Ordering::SeqCst) {
                trace!("repeater_process_tick_skip_running" = %id_for_log);
                return;
            }
            process_runner.spawn(
                state.clone(),
                spec.clone(),
                ProcessNotify::Silent,
                Some(id_for_log.clone()),
                id_for_log.clone(),
            );
        });
    }

    /// Mark OS-level repeat seen for id; stop software ticker repeats
    pub async fn note_os_repeat(&self, id: &str) {
        self.ticker.stop(id).await;
    }

    /// Stop any software repeats for `id`.
    pub async fn stop(&self, id: &str) {
        self.ticker.stop(id).await;
        let state = self.process_states.lock().remove(id);
        if let Some(state) = state
            && state.cancel_on_release
        {
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
            .process_states
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
        for state in self.process_states.lock().drain().map(|(_, state)| state) {
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
    async fn process_output_capture_is_bounded_and_marks_truncation() {
        let input = vec![b'x'; PROCESS_STREAM_LIMIT_BYTES + 4096];
        let capture = read_bounded(Cursor::new(input))
            .await
            .expect("read capture");

        assert_eq!(capture.bytes.len(), PROCESS_STREAM_LIMIT_BYTES);
        assert!(capture.truncated);
        let output = trim_process_output(
            capture,
            CapturedStream {
                bytes: Vec::new(),
                truncated: false,
            },
        );
        assert!(output.ends_with("[output truncated]"));
    }

    #[tokio::test]
    async fn direct_arguments_shell_language_and_non_utf8_output_share_runner() {
        let (tx, mut rx) = mpsc::channel(16);
        let notifier = crate::notification::NotificationDispatcher::new(tx);
        let repeater = Repeater::new(notifier);

        repeater.start(
            "literal-args".to_string(),
            ProcessSpec::new(
                "printf",
                vec![
                    "%s %s".to_string(),
                    "left right".to_string(),
                    "tail".to_string(),
                ],
                None,
                NotifyKind::Info,
                NotifyKind::Warn,
                "Process",
            ),
            None,
        );
        let literal = rx.recv().await.expect("literal process notification");
        assert!(matches!(
            literal,
            hotki_protocol::MsgToUI::Notify {
                kind: NotifyKind::Info,
                text,
                ..
            } if text == "left right tail"
        ));

        repeater.start(
            "shell-language".to_string(),
            ProcessSpec::shell(
                "value=right; printf '%s\\n' \"$value\" > /dev/null; printf 'left\\nright' | tr '[:lower:]' '[:upper:]' && printf '!'".to_string(),
                NotifyKind::Info,
                NotifyKind::Warn,
            ),
            None,
        );
        let shell = rx.recv().await.expect("shell process notification");
        assert!(matches!(
            shell,
            hotki_protocol::MsgToUI::Notify {
                kind: NotifyKind::Info,
                text,
                ..
            } if text == "LEFT\nRIGHT!"
        ));

        repeater.start(
            "non-utf8".to_string(),
            ProcessSpec::new(
                "/bin/sh",
                vec!["-c".to_string(), "printf '\\377'".to_string()],
                None,
                NotifyKind::Info,
                NotifyKind::Warn,
                "Process",
            ),
            None,
        );
        let non_utf8 = rx.recv().await.expect("non-UTF-8 process notification");
        assert!(matches!(
            non_utf8,
            hotki_protocol::MsgToUI::Notify {
                kind: NotifyKind::Info,
                text,
                ..
            } if text == "�"
        ));
    }

    #[tokio::test]
    async fn process_notifications_bound_output_and_report_spawn_context() {
        let (tx, mut rx) = mpsc::channel(16);
        let notifier = crate::notification::NotificationDispatcher::new(tx);
        let repeater = Repeater::new(notifier);

        let large = "x".repeat(PROCESS_STREAM_LIMIT_BYTES + 1);
        repeater.start(
            "large-output".to_string(),
            ProcessSpec::new(
                "/usr/bin/printf",
                vec!["%s".to_string(), large],
                None,
                NotifyKind::Info,
                NotifyKind::Warn,
                "Process",
            ),
            None,
        );
        let large = rx.recv().await.expect("large-output notification");
        assert!(matches!(
            large,
            hotki_protocol::MsgToUI::Notify {
                kind: NotifyKind::Info,
                text,
                ..
            } if text.ends_with("[output truncated]")
        ));

        repeater.start(
            "missing-program".to_string(),
            ProcessSpec::new(
                "hotki-program-that-does-not-exist",
                Vec::new(),
                None,
                NotifyKind::Ignore,
                NotifyKind::Info,
                "Process",
            ),
            None,
        );
        let missing = rx.recv().await.expect("missing-program notification");
        assert!(
            matches!(
                &missing,
                hotki_protocol::MsgToUI::Notify {
                    kind: NotifyKind::Info,
                    text,
                    ..
                } if text.contains("effective PATH=")
            ),
            "unexpected missing-program notification: {missing:?}"
        );

        repeater.start(
            "inherited-path".to_string(),
            ProcessSpec::new(
                "/bin/sh",
                vec!["-c".to_string(), "printf '%s' \"$PATH\"".to_string()],
                None,
                NotifyKind::Info,
                NotifyKind::Warn,
                "Process",
            ),
            None,
        );
        let inherited_path = rx.recv().await.expect("inherited PATH notification");
        let expected_path = env::var("PATH").unwrap_or_default();
        assert!(matches!(
            inherited_path,
            hotki_protocol::MsgToUI::Notify {
                kind: NotifyKind::Info,
                text,
                ..
            } if text == expected_path
        ));

        repeater.start(
            "invalid-cwd".to_string(),
            ProcessSpec::new(
                "true",
                Vec::new(),
                Some(PathBuf::from("/definitely/missing/hotki-directory")),
                NotifyKind::Ignore,
                NotifyKind::Info,
                "Process",
            ),
            None,
        );
        let invalid_cwd = rx.recv().await.expect("invalid-cwd notification");
        assert!(matches!(
            invalid_cwd,
            hotki_protocol::MsgToUI::Notify {
                kind: NotifyKind::Info,
                text,
                ..
            } if !text.contains("effective PATH=")
        ));
    }

    #[tokio::test]
    async fn one_shot_process_state_is_removed_after_completion() {
        let (tx, mut rx) = mpsc::channel(16);
        let notifier = crate::notification::NotificationDispatcher::new(tx);
        let repeater = Repeater::new(notifier);

        repeater.start(
            "one-shot-state".to_string(),
            ProcessSpec::new(
                "/usr/bin/true",
                Vec::new(),
                None,
                NotifyKind::Info,
                NotifyKind::Warn,
                "Process",
            ),
            None,
        );

        let _ = rx.recv().await.expect("one-shot notification");
        tokio::task::yield_now().await;
        assert!(
            !repeater
                .process_states
                .lock()
                .contains_key("one-shot-state")
        );
    }

    #[tokio::test]
    async fn stopping_process_action_terminates_owned_process_group() {
        let (tx, _rx) = mpsc::channel(16);
        let notifier = crate::notification::NotificationDispatcher::new(tx);
        let repeater = Repeater::new(notifier);

        repeater.start(
            "owned-shell".to_string(),
            ProcessSpec::shell(
                "sleep 30".to_string(),
                NotifyKind::Ignore,
                NotifyKind::Ignore,
            ),
            Some(RepeatSpec::default()),
        );
        let state = repeater.process_state("owned-shell", true);
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
        let (tx, _rx) = mpsc::channel(16);
        let notifier = crate::notification::NotificationDispatcher::new(tx);
        let repeater = Repeater::new(notifier);

        repeater.start(
            "one-shot-shell".to_string(),
            ProcessSpec::shell(
                "sleep 30".to_string(),
                NotifyKind::Ignore,
                NotifyKind::Ignore,
            ),
            None,
        );
        let state = repeater.process_state("one-shot-shell", false);
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
    async fn configured_process_action_delivers_captured_output() {
        let (tx, mut rx) = mpsc::channel(16);
        let notifier = crate::notification::NotificationDispatcher::new(tx);
        let repeater = Repeater::new(notifier);

        repeater.start(
            "captured-shell".to_string(),
            ProcessSpec::shell(
                "echo notify".to_string(),
                NotifyKind::Info,
                NotifyKind::Warn,
            ),
            None,
        );

        repeater.stop("captured-shell").await;
        let message = rx.recv().await.expect("process notification");
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
    async fn process_first_run_coalesces_then_unblocks_repeats() {
        let (tx, _rx) = mpsc::channel(16);
        let notifier = crate::notification::NotificationDispatcher::new(tx);
        let repeater = Repeater::new(notifier);

        let process_count = Arc::new(AtomicUsize::new(0));
        let process_count2 = process_count.clone();
        repeater.set_on_process_repeat(Arc::new(move |_id| {
            process_count2.fetch_add(1, AtomicOrdering::SeqCst);
        }));

        // Use a fast command, but hold the per-id gate so the initial run overlaps
        // the first repeat tick without relying on real time.
        let process = ProcessSpec::new(
            "/usr/bin/true",
            Vec::new(),
            None,
            config::NotifyKind::Ignore,
            config::NotifyKind::Ignore,
            "Process",
        );

        repeater.start(
            "shell-coalesce".to_string(),
            process,
            Some(RepeatSpec {
                initial_delay_ms: Some(100),
                interval_ms: Some(100),
            }),
        );

        // After ~150ms the first repeat tick would have fired; ensure it was coalesced
        let state = repeater.process_state("shell-coalesce", true);
        let gate_guard = state.gate.lock().await;
        tokio::task::yield_now().await; // let ticker + initial run tasks start and register timers
        advance(Duration::from_millis(150)).await;
        sleep(Duration::from_millis(0)).await; // yield so ticker task observes time advance
        assert_eq!(
            process_count.load(AtomicOrdering::SeqCst),
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
        while process_count.load(AtomicOrdering::SeqCst) == 0
            && real_start_repeat.elapsed() < StdDuration::from_secs(1)
        {
            tokio::task::yield_now().await;
        }
        let repeats = process_count.load(AtomicOrdering::SeqCst);
        repeater.stop("shell-coalesce").await;
        assert!(
            repeats >= 1,
            "At least one shell repeat should run after the first run completes",
        );
    }
}
