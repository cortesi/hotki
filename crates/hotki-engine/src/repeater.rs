//! Unified repeat system for shell commands and key relays.
//!
//! Manages the execution and repetition of actions triggered by hotkeys,
//! including both shell command execution and key relay forwarding.
//! Provides configurable timing and cancellation support.

use std::{
    cmp,
    collections::HashMap,
    env,
    option::Option,
    process::Command,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use config::NotifyKind;
use mac_keycode::Chord;
use parking_lot::Mutex;
use tokio::time::Duration;
use tracing::trace;

use crate::{
    RelayHandler, keymode::KeyResponse, notification::NotificationDispatcher, ticker::Ticker,
};

/// Maximum time to wait for a repeater task to acknowledge cancellation.
/// See repeater and ticker docs for semantics.
pub const STOP_WAIT_TIMEOUT_MS: u64 = 50;

/// Run a command using the user's shell, mapping output to a KeyResponse.
/// This function is blocking and intended to be called inside `spawn_blocking`.
fn run_shell_blocking(command: &str, ok_notify: NotifyKind, err_notify: NotifyKind) -> KeyResponse {
    tracing::info!("Executing shell command: {}", command);
    let shell_path: String = env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string());
    match Command::new(&shell_path).arg("-lc").arg(command).output() {
        Ok(output) => {
            // Unify stdout and stderr into a single message
            let mut combined = String::new();
            let out = String::from_utf8_lossy(&output.stdout);
            let err = String::from_utf8_lossy(&output.stderr);
            if !out.is_empty() {
                combined.push_str(&out);
            }
            if !err.is_empty() {
                if !combined.is_empty() && !combined.ends_with('\n') {
                    combined.push('\n');
                }
                combined.push_str(&err);
            }

            // Trim blank lines at the start and end
            let combined = {
                let lines: Vec<&str> = combined.lines().collect();
                let first_nonblank = lines.iter().position(|l| !l.trim().is_empty());
                let last_nonblank = lines.iter().rposition(|l| !l.trim().is_empty());
                match (first_nonblank, last_nonblank) {
                    (Some(s), Some(e)) if s <= e => lines[s..=e].join("\n"),
                    _ => String::new(),
                }
            };

            // Choose notification type based on exit status
            let notify_type = if output.status.success() {
                ok_notify
            } else {
                err_notify
            };

            match notify_type {
                NotifyKind::Ignore => KeyResponse::Ok,
                NotifyKind::Info => KeyResponse::Info {
                    title: "Shell command".to_string(),
                    text: combined,
                },
                NotifyKind::Warn => KeyResponse::Warn {
                    title: "Shell command".to_string(),
                    text: combined,
                },
                NotifyKind::Error => KeyResponse::Error {
                    title: "Shell command".to_string(),
                    text: combined,
                },
                NotifyKind::Success => KeyResponse::Success {
                    title: "Shell command".to_string(),
                    text: combined,
                },
            }
        }
        Err(e) => KeyResponse::Warn {
            title: "Shell command".to_string(),
            text: format!("Failed to execute: {}", e),
        },
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
    /// Focus context providing current (app, title, pid).
    focus_ctx: Arc<Mutex<Option<(String, String, i32)>>>,
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
        focus_ctx: Arc<Mutex<Option<(String, String, i32)>>>,
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
            ticker: Ticker::new(),
            on_relay_repeat: Arc::new(Mutex::new(None)),
            on_shell_repeat: Arc::new(Mutex::new(None)),
            shell_states: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Get the current focused PID, or -1 if unknown.
    fn current_pid(&self) -> i32 {
        self.focus_ctx.lock().as_ref().map(|t| t.2).unwrap_or(-1)
    }

    /* tests moved to end of module */

    /// Get or create the per-id shell run state.
    fn shell_state(&self, id: &str) -> Arc<ShellRunState> {
        let mut map = self.shell_states.lock();
        if let Some(s) = map.get(id) {
            return s.clone();
        }
        let state = Arc::new(ShellRunState {
            gate: tokio::sync::Mutex::new(()),
            running: AtomicBool::new(false),
        });
        map.insert(id.to_string(), state.clone());
        state
    }

    fn effective_timings(&self, spec: Option<RepeatSpec>) -> (Duration, Duration) {
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

    /// Optional: install a relay repeat callback for instrumentation/testing.
    pub fn set_on_relay_repeat(&self, cb: OnRelayRepeat) {
        *self.on_relay_repeat.lock() = Some(cb);
    }

    /// Optional: install a shell repeat callback for instrumentation/testing.
    pub fn set_on_shell_repeat(&self, cb: OnShellRepeat) {
        *self.on_shell_repeat.lock() = Some(cb);
    }

    /// Convenience helper for relay repeating start (testing/tools)
    pub fn start_relay_repeat(&self, id: String, chord: Chord, repeat: Option<RepeatSpec>) {
        self.start(id, ExecSpec::Relay { chord }, repeat);
    }

    /// Convenience helper for shell repeating start (testing/tools)
    pub fn start_shell_repeat(&self, id: String, command: String, repeat: Option<RepeatSpec>) {
        self.stop(&id);

        // First run (notifications ignored)
        let _ = self.notifier.handle_key_response(KeyResponse::ShellAsync {
            command: command.clone(),
            ok_notify: NotifyKind::Ignore,
            err_notify: NotifyKind::Ignore,
            repeat: None,
        });

        if repeat.is_some() {
            self.spawn_shell_repeater(id, command, repeat);
        }
    }

    /// Start execution for a binding id. Runs the first action immediately and schedules repeats if provided.
    pub fn start(&self, id: String, exec: ExecSpec, repeat: Option<RepeatSpec>) {
        self.stop(&id); // replace if exists

        match exec.clone() {
            ExecSpec::Shell {
                command,
                ok_notify,
                err_notify,
            } => {
                // First run with notifications via executor, serialized per id.
                let notifier = self.notifier.clone();
                let cmd = command.clone();
                let state = self.shell_state(&id);
                // Mark running to coalesce any immediate repeat tick.
                state.running.store(true, Ordering::SeqCst);
                tokio::spawn(async move {
                    let _guard = state.gate.lock().await;
                    let resp = tokio::task::spawn_blocking(move || {
                        run_shell_blocking(&cmd, ok_notify, err_notify)
                    })
                    .await
                    .unwrap_or_else(|e| {
                        tracing::warn!("Shell task join error: {}", e);
                        KeyResponse::Warn {
                            title: "Shell command".to_string(),
                            text: "Execution task failed".to_string(),
                        }
                    });
                    if let Err(e) = notifier.handle_key_response(resp) {
                        tracing::warn!("Failed to deliver shell response: {}", e);
                    }
                    state.running.store(false, Ordering::SeqCst);
                });

                // Schedule silent repeats if requested
                if repeat.is_some() {
                    self.spawn_shell_repeater(id, command, repeat);
                }
            }
            ExecSpec::Relay { chord } => {
                // Start relay immediately (non-repeat)
                let pid = self.current_pid();
                self.relay
                    .start_relay(id.clone(), chord.clone(), pid, false);

                if repeat.is_some() {
                    self.spawn_relay_repeater(id, chord, pid, repeat);
                }
            }
        }
    }

    fn spawn_shell_repeater(&self, id: String, command: String, repeat: Option<RepeatSpec>) {
        let (initial_delay, interval) = self.effective_timings(repeat);
        let id_for_log = id.clone();
        let state = self.shell_state(&id_for_log);

        let on_shell_repeat = self.on_shell_repeat.clone();
        self.ticker.start(id, initial_delay, interval, move || {
            // Skip if a prior run is still active
            if state.running.swap(true, Ordering::SeqCst) {
                trace!("repeater_shell_tick_skip_running" = %id_for_log);
                return;
            }
            let cmd = command.clone();
            let id_for_trace = id_for_log.clone();
            // Spawn async task to serialize via per-id async mutex, then run blocking
            let on_shell_repeat2 = on_shell_repeat.clone();
            let state2 = state.clone();
            tokio::spawn(async move {
                let _guard = state2.gate.lock().await;
                let _ = tokio::task::spawn_blocking(move || {
                    run_shell_blocking(&cmd, NotifyKind::Ignore, NotifyKind::Ignore)
                })
                .await;
                // Notify shell repeat callback if set
                if let Some(cb) = on_shell_repeat2.lock().as_ref() {
                    cb(&id_for_trace);
                }
                state2.running.store(false, Ordering::SeqCst);
                trace!("repeater_shell_run_done" = %id_for_trace);
            });
        });
    }

    fn spawn_relay_repeater(
        &self,
        id: String,
        chord: Chord,
        initial_pid: i32,
        repeat: Option<RepeatSpec>,
    ) {
        let (initial_delay, interval) = self.effective_timings(repeat);
        let relay = self.relay.clone();
        let focus_ctx = self.focus_ctx.clone();
        let id_for_log = id.clone();
        let ch = chord.clone();

        let mut last_pid = initial_pid;
        let on_relay_repeat = self.on_relay_repeat.clone();
        // Coalesce relay repeats to avoid overlapping enqueueing under jitter
        let running = Arc::new(AtomicBool::new(false));
        let running_flag = running.clone();
        self.ticker.start(id, initial_delay, interval, move || {
            let pid = focus_ctx.lock().as_ref().map(|t| t.2).unwrap_or(-1);
            if pid != -1 && pid != last_pid {
                // Handoff: Up old, Down new (non-repeat)
                relay.stop_relay(&id_for_log, last_pid);
                relay.start_relay(id_for_log.clone(), ch.clone(), pid, false);
                last_pid = pid;
                return;
            }
            if pid != -1 {
                // Skip if a prior repeat is still running
                if running_flag.swap(true, Ordering::SeqCst) {
                    trace!("repeater_relay_tick_skip_running" = %id_for_log);
                    return;
                }
                let _ = relay.repeat_relay(&id_for_log, pid);
                // Notify relay repeat callback if set
                if let Some(cb) = on_relay_repeat.lock().as_ref() {
                    cb(&id_for_log);
                }
                running_flag.store(false, Ordering::SeqCst);
            }
        });
    }

    /// Mark OS-level repeat seen for id; stop software ticker repeats
    pub fn note_os_repeat(&self, id: &str) {
        self.ticker.stop_sync(id);
    }

    /// Stop any software repeats for `id`.
    pub fn stop(&self, id: &str) {
        self.ticker.stop(id);
        // Best-effort cleanup of per-id shell state
        let _ = self.shell_states.lock().remove(id);
    }

    /// Stop and wait briefly for repeats for `id` to finish.
    pub fn stop_sync(&self, id: &str) {
        self.ticker.stop_sync(id);
        // Best-effort cleanup of per-id shell state
        let _ = self.shell_states.lock().remove(id);
    }

    /// Returns true if the ticker is currently running for `id`.
    pub fn is_ticking(&self, id: &str) -> bool {
        self.ticker.is_active(id)
    }

    // No public callback constructor is exposed; registration is managed by KeyBindingManager.

    /// Stop all tickers and wait briefly for completion.
    pub fn clear_sync(&self) {
        self.ticker.clear_sync();
    }

    /// Stop all tickers asynchronously and wait briefly for completion.
    pub async fn clear_async(&self) {
        self.ticker.clear_async().await;
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};

    use tokio::{
        sync::mpsc,
        time::{Duration, advance, sleep},
    };

    use super::*;

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn shell_first_run_coalesces_then_unblocks_repeats() {
        let focus_ctx = Arc::new(Mutex::new(None::<(String, String, i32)>));
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
        let state = repeater.shell_state("shell-coalesce");
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
        let real_start = std::time::Instant::now();
        while state.running.load(AtomicOrdering::SeqCst)
            && real_start.elapsed() < std::time::Duration::from_secs(1)
        {
            tokio::task::yield_now().await;
        }
        assert!(
            !state.running.load(AtomicOrdering::SeqCst),
            "initial shell run should complete"
        );

        // Advance far enough for the next repeat ticks to execute after initial completion.
        advance(Duration::from_millis(250)).await;
        let real_start_repeat = std::time::Instant::now();
        while shell_count.load(AtomicOrdering::SeqCst) == 0
            && real_start_repeat.elapsed() < std::time::Duration::from_secs(1)
        {
            tokio::task::yield_now().await;
        }
        let repeats = shell_count.load(AtomicOrdering::SeqCst);
        repeater.stop_sync("shell-coalesce");
        assert!(
            repeats >= 1,
            "At least one shell repeat should run after the first run completes",
        );
    }
}
