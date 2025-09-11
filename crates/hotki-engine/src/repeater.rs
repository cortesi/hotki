//! Unified repeat system for shell commands and key relays.
//!
//! Manages the execution and repetition of actions triggered by hotkeys,
//! including both shell command execution and key relay forwarding.
//! Provides configurable timing and cancellation support.

use std::{
    cmp, env,
    process::Command,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use tokio::time::Duration;
use tracing::trace;

use keymode::{KeyResponse, NotificationType};
use mac_keycode::Chord;

use crate::{RelayHandler, notification::NotificationDispatcher, ticker::Ticker};
use std::option::Option;

/// Maximum time to wait for a repeater task to acknowledge cancellation.
/// See repeater and ticker docs for semantics.
pub const STOP_WAIT_TIMEOUT_MS: u64 = 50;

/// Run a command using the user's shell, mapping output to a KeyResponse.
/// This function is blocking and intended to be called inside `spawn_blocking`.
fn run_shell_blocking(
    command: &str,
    ok_notify: NotificationType,
    err_notify: NotificationType,
) -> KeyResponse {
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
                NotificationType::Ignore => KeyResponse::Ok,
                NotificationType::Info => KeyResponse::Info {
                    title: "Shell command".to_string(),
                    text: combined,
                },
                NotificationType::Warn => KeyResponse::Warn {
                    title: "Shell command".to_string(),
                    text: combined,
                },
                NotificationType::Error => KeyResponse::Error {
                    title: "Shell command".to_string(),
                    text: combined,
                },
                NotificationType::Success => KeyResponse::Success {
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
pub const REPEAT_MIN_INITIAL_DELAY_MS: u64 = 100;
pub const REPEAT_MAX_INITIAL_DELAY_MS: u64 = 1000;
pub const REPEAT_MIN_INTERVAL_MS: u64 = 100;
pub const REPEAT_MAX_INTERVAL_MS: u64 = 2000;
pub const REPEAT_DEFAULT_MIN_INTERVAL_MS: u64 = 150;

/// System default timings for key repeat
const SYS_INITIAL_DELAY_MS: u64 = 250; // Initial delay before first repeat
const SYS_INTERVAL_MS: u64 = 33; // ~30 repeats per second

/// What to execute (first run is immediate; repeats are driven by the repeater)
#[derive(Clone)]
pub enum ExecSpec {
    Shell {
        command: String,
        ok_notify: NotificationType,
        err_notify: NotificationType,
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

/// Unified repeater that runs first-run immediately and then repeats while held
#[derive(Clone)]
pub struct Repeater {
    sys_initial: Duration,
    sys_interval: Duration,
    focus_pid: Arc<Mutex<Option<i32>>>, // read-only provider for current pid
    relay: RelayHandler,
    notifier: NotificationDispatcher,
    ticker: Ticker,
    repeat_observer: Arc<Mutex<Option<Arc<dyn RepeatObserver>>>>,
}

/// Observer interface for repeat ticks (used by tests/tools)
pub trait RepeatObserver: Send + Sync {
    /// Called whenever a relay (key) repeat tick fires for the given id.
    fn on_relay_repeat(&self, _id: &str) {}
    /// Called whenever a shell repeat tick fires for the given id.
    fn on_shell_repeat(&self, _id: &str) {}
}

impl Repeater {
    /// Create a new repeater bound to the given focus/relay/notifier components.
    pub fn new(
        focus_pid: Arc<Mutex<Option<i32>>>,
        relay: RelayHandler,
        notifier: NotificationDispatcher,
    ) -> Self {
        let sys_initial = Duration::from_millis(SYS_INITIAL_DELAY_MS);
        let sys_interval = Duration::from_millis(SYS_INTERVAL_MS);
        Self {
            sys_initial,
            sys_interval,
            focus_pid,
            relay,
            notifier,
            ticker: Ticker::new(),
            repeat_observer: Arc::new(Mutex::new(None)),
        }
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

    /// Optional: install a repeat observer used for instrumentation/testing
    pub fn set_repeat_observer(&self, obs: Arc<dyn RepeatObserver>) {
        match self.repeat_observer.lock() {
            Ok(mut guard) => *guard = Some(obs),
            Err(e) => {
                tracing::error!("Failed to set repeat observer: {}", e);
            }
        }
    }

    /// Convenience helper for relay repeating start (testing/tools)
    pub fn start_relay_repeat(&self, id: String, chord: Chord, repeat: Option<RepeatSpec>) {
        self.start(id, ExecSpec::Relay { chord }, repeat);
    }

    /// Convenience helper for shell repeating start (testing/tools)
    pub fn start_shell_repeat(&self, id: String, command: String, repeat: Option<RepeatSpec>) {
        self.stop(&id);

        // First run (notifications ignored)
        let _ = self
            .notifier
            .handle_key_response(keymode::KeyResponse::ShellAsync {
                command: command.clone(),
                ok_notify: NotificationType::Ignore,
                err_notify: NotificationType::Ignore,
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
                // First run with notifications via executor
                let notifier = self.notifier.clone();
                let cmd = command.clone();
                tokio::task::spawn_blocking(move || {
                    let resp = run_shell_blocking(&cmd, ok_notify, err_notify);
                    if let Err(e) = notifier.handle_key_response(resp) {
                        tracing::warn!("Failed to deliver shell response: {}", e);
                    }
                });

                // Schedule silent repeats if requested
                if repeat.is_some() {
                    self.spawn_shell_repeater(id, command, repeat);
                }
            }
            ExecSpec::Relay { chord } => {
                // Start relay immediately (non-repeat)
                let pid = self.focus_pid.lock().ok().and_then(|g| *g).unwrap_or(-1);
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
        let running = Arc::new(AtomicBool::new(false));
        let running_task = running.clone();

        let rep_obs = self.repeat_observer.clone();
        self.ticker.start(id, initial_delay, interval, move || {
            // Skip if a prior run is still active
            if running_task.swap(true, Ordering::SeqCst) {
                trace!("repeater_shell_tick_skip_running" = %id_for_log);
                return;
            }
            let cmd = command.clone();
            let running_clear = running_task.clone();
            let id_for_trace = id_for_log.clone();
            // Spawn blocking and return immediately to allow coalescing/skip behavior
            let rep_obs2 = rep_obs.clone();
            tokio::task::spawn_blocking(move || {
                let _ =
                    run_shell_blocking(&cmd, NotificationType::Ignore, NotificationType::Ignore);
                // Note shell repeat for observers
                let obs = match rep_obs2.lock() {
                    Ok(guard) => guard.as_ref().cloned(),
                    Err(e) => {
                        tracing::error!("Failed to lock repeat observer: {}", e);
                        None
                    }
                };
                if let Some(obs) = obs {
                    obs.on_shell_repeat(&id_for_trace);
                }
                running_clear.store(false, Ordering::SeqCst);
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
        let focus_pid = self.focus_pid.clone();
        let id_for_log = id.clone();
        let ch = chord.clone();

        let mut last_pid = initial_pid;
        let rep_obs = self.repeat_observer.clone();
        // Coalesce relay repeats to avoid overlapping enqueueing under jitter
        let running = Arc::new(AtomicBool::new(false));
        let running_flag = running.clone();
        self.ticker.start(id, initial_delay, interval, move || {
            let pid = focus_pid.lock().ok().and_then(|g| *g).unwrap_or(-1);
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
                // Note relay repeat for observers
                let obs = match rep_obs.lock() {
                    Ok(guard) => guard.as_ref().cloned(),
                    Err(e) => {
                        tracing::error!("Failed to lock repeat observer: {}", e);
                        None
                    }
                };
                if let Some(obs) = obs {
                    obs.on_relay_repeat(&id_for_log);
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
    }

    /// Stop and wait briefly for repeats for `id` to finish.
    pub fn stop_sync(&self, id: &str) {
        self.ticker.stop_sync(id);
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
