use std::{
    any::Any,
    collections::BTreeSet,
    fs,
    panic::{self, AssertUnwindSafe},
    path::{Path, PathBuf},
    process::exit,
    result::Result as StdResult,
    sync::mpsc::{self, RecvTimeoutError},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use serde::Serialize;
use tracing::info;

use crate::{
    cases,
    error::{Error, Result, print_hints},
    process,
    warn_overlay::OverlaySession,
};

/// Common tracing target for smoketest case logging.
pub const LOG_TARGET: &str = "smoketest.case";

/// Convert a case slug into the canonical filename prefix.
pub fn sanitize_slug(slug: &str) -> String {
    slug.chars()
        .map(|ch| match ch {
            '.' | '-' => '_',
            other => other,
        })
        .collect()
}

/// Retrieve a case registry entry by slug.
pub fn case_by_slug(slug: &str) -> Option<&'static CaseEntry> {
    CASES.iter().find(|entry| entry.name == slug)
}

/// Optional overrides applied when running registry-backed cases.
#[derive(Clone, Copy, Default)]
pub struct CaseRunOpts {
    /// Override whether the warn overlay should be shown during the run.
    pub warn_overlay: Option<bool>,
    /// Override the fail-fast behavior for the runner configuration.
    pub fail_fast: Option<bool>,
}

/// Configured time budget for a smoketest case.
#[derive(Clone, Copy, Debug, Serialize)]
pub struct Budget {
    /// Maximum milliseconds expected in the setup stage.
    pub setup_ms: u64,
    /// Maximum milliseconds expected in the action stage.
    pub action_ms: u64,
    /// Maximum milliseconds expected while waiting for the case to settle.
    pub settle_ms: u64,
}

impl Budget {
    /// Construct a budget from setup/action/settle millisecond values.
    pub const fn new(setup_ms: u64, action_ms: u64, settle_ms: u64) -> Self {
        Self {
            setup_ms,
            action_ms,
            settle_ms,
        }
    }
}

/// Documentation for helper functions that a case relies on.
#[derive(Clone, Copy, Debug)]
pub struct HelperDoc {
    /// Name of the helper function.
    pub name: &'static str,
    /// Short description surfaced in docs.
    pub summary: &'static str,
}

/// Registry entry describing a smoketest case.
pub struct CaseEntry {
    /// Registry slug used for CLI dispatch and artifact naming.
    pub name: &'static str,
    /// Optional description surfaced in headings and overlay text.
    pub info: Option<&'static str>,
    /// Whether the case body must run on the main thread.
    pub main_thread: bool,
    /// Additional watchdog headroom appended to the base timeout.
    pub extra_timeout_ms: u64,
    /// Expected timing budget for each stage.
    pub budget: Budget,
    /// Helper API surface consumed by the case.
    pub helpers: &'static [HelperDoc],
    /// Function pointer invoked to execute the case.
    pub run: fn(&mut CaseCtx<'_>) -> Result<()>,
}

impl CaseEntry {
    /// Create a main-thread case entry with the provided metadata.
    pub const fn main(
        name: &'static str,
        info: Option<&'static str>,
        extra_timeout_ms: u64,
        budget: Budget,
        helpers: &'static [HelperDoc],
        run: fn(&mut CaseCtx<'_>) -> Result<()>,
    ) -> Self {
        Self {
            name,
            info,
            main_thread: true,
            extra_timeout_ms,
            budget,
            helpers,
            run,
        }
    }

    /// Create a background-thread case entry with the provided metadata.
    pub const fn background(
        name: &'static str,
        info: Option<&'static str>,
        extra_timeout_ms: u64,
        budget: Budget,
        helpers: &'static [HelperDoc],
        run: fn(&mut CaseCtx<'_>) -> Result<()>,
    ) -> Self {
        Self {
            main_thread: false,
            ..Self::main(name, info, extra_timeout_ms, budget, helpers, run)
        }
    }
}

/// Optional per-stage timings captured during case execution.
#[derive(Clone, Copy, Debug, Default, Serialize)]
pub struct StageDurationsOptional {
    /// Setup duration, when recorded.
    pub setup_ms: Option<u64>,
    /// Action duration, when recorded.
    pub action_ms: Option<u64>,
    /// Settle duration, when recorded.
    pub settle_ms: Option<u64>,
}

impl StageDurationsOptional {
    /// Record the elapsed time for the provided stage.
    fn set(&mut self, stage: Stage, duration_ms: u64) -> Result<()> {
        let slot = match stage {
            Stage::Setup => &mut self.setup_ms,
            Stage::Action => &mut self.action_ms,
            Stage::Settle => &mut self.settle_ms,
        };
        if slot.is_some() {
            return Err(Error::InvalidState(format!(
                "stage {:?} executed more than once",
                stage
            )));
        }
        *slot = Some(duration_ms);
        Ok(())
    }
}

/// Execution stage identifiers used for timing measurements.
#[derive(Clone, Copy, Debug)]
pub enum Stage {
    /// Initial setup step (spawning helpers, initial probes).
    Setup,
    /// Primary action step (issuing commands to the world).
    Action,
    /// Final settle step (waiting for convergence and asserting results).
    Settle,
}

/// Mutable case context that accumulates stage timings and scratch metadata.
pub struct CaseCtx<'a> {
    /// Registry slug associated with the current case.
    name: &'a str,
    /// Scratch directory allocated for the case.
    scratch_dir: PathBuf,
    /// Optional stage timings recorded during execution.
    durations: StageDurationsOptional,
}

impl<'a> CaseCtx<'a> {
    /// Construct a new case context with the provided identifiers.
    pub fn new(name: &'a str, scratch_dir: PathBuf) -> Self {
        Self {
            name,
            scratch_dir,
            durations: StageDurationsOptional::default(),
        }
    }

    /// Execute the provided stage closure and capture its elapsed duration.
    pub fn stage<F, T>(&mut self, stage: Stage, f: F) -> Result<T>
    where
        F: FnOnce(&mut CaseCtx<'_>) -> Result<T>,
    {
        let start = Instant::now();
        let result = f(self);
        let elapsed_ms = start.elapsed().as_millis() as u64;
        self.durations.set(stage, elapsed_ms)?;
        result
    }

    /// Consume the context and return recorded stage durations.
    pub(crate) fn into_durations(self) -> StageDurationsOptional {
        self.durations
    }

    /// Run the setup stage and record the elapsed duration.
    pub fn setup<F, T>(&mut self, f: F) -> Result<T>
    where
        F: FnOnce(&mut CaseCtx<'_>) -> Result<T>,
    {
        self.stage(Stage::Setup, f)
    }

    /// Run the action stage and record the elapsed duration.
    pub fn action<F, T>(&mut self, f: F) -> Result<T>
    where
        F: FnOnce(&mut CaseCtx<'_>) -> Result<T>,
    {
        self.stage(Stage::Action, f)
    }

    /// Run the settle stage and record the elapsed duration.
    pub fn settle<F, T>(&mut self, f: F) -> Result<T>
    where
        F: FnOnce(&mut CaseCtx<'_>) -> Result<T>,
    {
        self.stage(Stage::Settle, f)
    }

    /// Build a scratch path relative to the case directory.
    pub fn scratch_path<P: AsRef<Path>>(&self, relative_path: P) -> PathBuf {
        self.scratch_dir.join(relative_path.as_ref())
    }

    /// Log a structured event associated with this case.
    pub fn log_event(&self, label: &str, message: &str) {
        info!(
            target: LOG_TARGET,
            event = label,
            case = self.name,
            message = message
        );
    }
}

/// Execution settings shared by registry-driven smoketest runs.
pub struct RunnerConfig<'a> {
    /// Suppress headings and non-error output when `true`.
    pub quiet: bool,
    /// Whether to show the hands-off overlay while running cases.
    pub warn_overlay: bool,
    /// Base timeout used for each case before per-entry adjustments.
    pub base_timeout_ms: u64,
    /// Stop after the first failure when set.
    pub fail_fast: bool,
    /// Optional overlay text displayed below the case name.
    pub overlay_info: Option<&'a str>,
}

/// Summary of a single case run emitted by the registry runner.
pub struct CaseRunOutcome {
    /// Registry entry executed for the case.
    pub entry: &'static CaseEntry,
    /// Wall-clock duration spent running the case body (including watchdog).
    pub elapsed: Duration,
    /// Primary execution error, if any, returned by the case body.
    pub primary_error: Option<Error>,
    /// Quiescence check failure raised during teardown, if any.
    pub quiescence_error: Option<Error>,
}

impl CaseRunOutcome {
    /// Returns true when the case run produced neither primary nor cleanup errors.
    fn is_success(&self) -> bool {
        self.primary_error.is_none() && self.quiescence_error.is_none()
    }

    /// Returns true when any error surfaced during the case run.
    fn is_failure(&self) -> bool {
        !self.is_success()
    }
}

/// Summary emitted after executing a smoketest case body.
struct CaseExecution {
    /// Wall-clock duration consumed by the case run.
    elapsed: Duration,
    /// Primary error returned by the case body, if any.
    primary_error: Option<Error>,
}

/// Run a registry entry with pre/post world cleanup guards.
fn run_case(
    entry: &'static CaseEntry,
    config: &RunnerConfig<'_>,
    scratch_root: &Path,
    index: usize,
) -> Result<CaseRunOutcome> {
    let case_dir = create_case_dir(scratch_root, index, entry.name)?;
    let execution = execute_case(entry, config, case_dir);

    Ok(CaseRunOutcome {
        entry,
        elapsed: execution.elapsed,
        primary_error: execution.primary_error,
        quiescence_error: None,
    })
}

/// Shut down shared overlay state and surface aggregated failure information.
/// Run the provided case sequence with shared suite setup and teardown.
fn run_suite<I>(config: &RunnerConfig<'_>, cases: I, announce_success: bool) -> Result<()>
where
    I: IntoIterator<Item = (usize, &'static CaseEntry)>,
{
    ensure_helper_limit()?;
    let scratch_root = create_scratch_root()?;
    let overlay = if config.warn_overlay && !config.quiet {
        OverlaySession::start()
    } else {
        None
    };
    let mut failures = Vec::new();
    for (idx, entry) in cases.into_iter() {
        if !config.quiet {
            crate::heading(&format!("Test: {}", entry.name));
        }
        if let Some(session) = overlay.as_ref() {
            let info = config.overlay_info.or(entry.info).unwrap_or("");
            session.set_info(info);
            session.set_status(entry.name);
        }
        let outcome = run_case(entry, config, &scratch_root, idx)?;
        report_outcome(&outcome, config.quiet);
        if outcome.is_failure() {
            failures.push(entry);
            if config.fail_fast {
                break;
            }
        }
    }
    if failures.is_empty() {
        if announce_success && !config.quiet {
            println!("All smoketest cases passed");
        }
        Ok(())
    } else {
        let names = failures
            .iter()
            .map(|entry| entry.name)
            .collect::<Vec<_>>()
            .join(", ");
        Err(Error::InvalidState(format!(
            "smoketest cases failed: {names}"
        )))
    }
}

/// Run every registered case in order, respecting the supplied runner configuration.
pub fn run_all(config: &RunnerConfig<'_>) -> Result<()> {
    run_suite(config, CASES.iter().enumerate(), true)
}

/// Run a subset of cases by name, preserving the CLI sequence order.
pub fn run_sequence(names: &[&str], config: &RunnerConfig<'_>) -> Result<()> {
    let ordered = resolve_named_cases(names)?;
    run_suite(config, ordered, false)
}

/// Resolve CLI-provided case names into registry entries paired with sequence order.
fn resolve_named_cases(names: &[&str]) -> Result<Vec<(usize, &'static CaseEntry)>> {
    names
        .iter()
        .enumerate()
        .map(|(idx, name)| {
            let entry = case_by_slug(name)
                .ok_or_else(|| Error::InvalidState(format!("unknown smoketest case: {name}")))?;
            Ok((idx, entry))
        })
        .collect()
}

/// Execute a single registry entry and capture timing/artifact metadata.
fn execute_case(
    entry: &'static CaseEntry,
    config: &RunnerConfig<'_>,
    case_dir: PathBuf,
) -> CaseExecution {
    let budget_total = entry
        .budget
        .setup_ms
        .saturating_add(entry.budget.action_ms)
        .saturating_add(entry.budget.settle_ms);
    let timeout_ms = budget_total
        .saturating_add(config.base_timeout_ms)
        .saturating_add(entry.extra_timeout_ms);

    let start = Instant::now();
    let exec_result = if entry.main_thread {
        let case_dir_clone = case_dir.clone();
        run_on_main_with_watchdog(entry.name, timeout_ms, move || {
            let mut ctx = CaseCtx::new(entry.name, case_dir_clone);
            let result = (entry.run)(&mut ctx);
            (ctx, result)
        })
    } else {
        run_with_watchdog(entry.name, timeout_ms, move || {
            let mut ctx = CaseCtx::new(entry.name, case_dir);
            let result = (entry.run)(&mut ctx);
            (ctx, result)
        })
    };
    let elapsed = start.elapsed();

    match exec_result {
        Ok((ctx, run_result)) => {
            let durations = ctx.into_durations();
            log_case_timing(entry, &durations);
            CaseExecution {
                elapsed,
                primary_error: run_result.err(),
            }
        }
        Err(abort) => CaseExecution {
            elapsed,
            primary_error: Some(abort.into_error()),
        },
    }
}

/// Emit a structured log entry summarizing configured budgets and observed durations for a case.
fn log_case_timing(entry: &CaseEntry, durations: &StageDurationsOptional) {
    info!(
        target: LOG_TARGET,
        event = "stage_timings",
        case = entry.name,
        setup_budget_ms = entry.budget.setup_ms,
        action_budget_ms = entry.budget.action_ms,
        settle_budget_ms = entry.budget.settle_ms,
        setup_ms = durations.setup_ms,
        action_ms = durations.action_ms,
        settle_ms = durations.settle_ms
    );
}

/// Print a user-friendly summary for the supplied outcome.
fn report_outcome(outcome: &CaseRunOutcome, quiet: bool) {
    let elapsed = outcome.elapsed.as_secs_f64();
    if outcome.is_success() {
        if !quiet {
            println!("{}... OK ({elapsed:.3}s)", outcome.entry.name);
        }
        return;
    }

    println!("{}... FAIL ({elapsed:.3}s)", outcome.entry.name);
    if let Some(err) = &outcome.primary_error {
        eprintln!("{}: {}", outcome.entry.name, err);
        print_hints(err);
    }
    if let Some(err) = &outcome.quiescence_error {
        eprintln!("{}: cleanup error: {}", outcome.entry.name, err);
    }
}

/// Abort reason surfaced by watchdog wrappers around case execution.
#[derive(Debug)]
enum WatchdogAbort {
    /// The watchdog deadline elapsed before the worker finished.
    Timeout {
        /// Identifier for the task guarded by the watchdog.
        name: String,
        /// Timeout budget in milliseconds assigned to the task.
        timeout_ms: u64,
    },
    /// The worker panicked while the watchdog was armed.
    Panic {
        /// Identifier for the task guarded by the watchdog.
        name: String,
        /// Message extracted from the panic payload.
        message: String,
    },
}

impl WatchdogAbort {
    /// Build a timeout abort using the provided task name and deadline.
    fn timeout(name: &str, timeout_ms: u64) -> Self {
        Self::Timeout {
            name: name.to_string(),
            timeout_ms,
        }
    }

    /// Build a panic abort from a plain string message.
    fn panic_from_message(name: &str, message: String) -> Self {
        Self::Panic {
            name: name.to_string(),
            message,
        }
    }

    /// Translate the abort into the suite's error type.
    fn into_error(self) -> Error {
        match self {
            Self::Timeout { name, timeout_ms } => {
                Error::InvalidState(format!("watchdog timeout after {timeout_ms} ms in {name}"))
            }
            Self::Panic { name, message } => {
                Error::InvalidState(format!("test case {name} panicked: {message}"))
            }
        }
    }
}

/// Render a human-readable message from a panic payload for diagnostics.
fn panic_message(payload: &(dyn Any + Send + 'static)) -> String {
    if let Some(msg) = payload.downcast_ref::<&str>() {
        (*msg).to_string()
    } else if let Some(msg) = payload.downcast_ref::<String>() {
        msg.clone()
    } else {
        "unknown panic payload".to_string()
    }
}

/// Convenience alias for watchdog guard results.
type WatchdogResult<T> = StdResult<T, WatchdogAbort>;

/// Outcome produced by the worker thread controlled by the watchdog.
enum WorkerOutcome<T> {
    /// Worker completed successfully with the provided value.
    Completed(T),
    /// Worker panicked and yielded the formatted panic message.
    Panicked(String),
}

/// Run `f` on a background thread and bail out if the timeout expires.
fn run_with_watchdog<F, T>(name: &str, timeout_ms: u64, f: F) -> WatchdogResult<T>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let outcome = panic::catch_unwind(AssertUnwindSafe(f)).map_or_else(
            |payload| WorkerOutcome::Panicked(panic_message(payload.as_ref())),
            WorkerOutcome::Completed,
        );
        if tx.send(outcome).is_err() {
            // Receiver was dropped; the watchdog will see the disconnect.
        }
    });

    match rx.recv_timeout(Duration::from_millis(timeout_ms)) {
        Ok(WorkerOutcome::Completed(value)) => Ok(value),
        Ok(WorkerOutcome::Panicked(message)) => {
            Err(WatchdogAbort::panic_from_message(name, message))
        }
        Err(RecvTimeoutError::Timeout) => {
            eprintln!(
                "ERROR: smoketest watchdog timeout ({} ms) in {} — force exiting",
                timeout_ms, name
            );
            process::kill_all();
            Err(WatchdogAbort::timeout(name, timeout_ms))
        }
        Err(RecvTimeoutError::Disconnected) => {
            process::kill_all();
            Err(WatchdogAbort::panic_from_message(
                name,
                "worker thread disconnected".to_string(),
            ))
        }
    }
}

/// Run `f` on the current thread while a watchdog enforces the deadline.
fn run_on_main_with_watchdog<F, T>(name: &str, timeout_ms: u64, f: F) -> WatchdogResult<T>
where
    F: FnOnce() -> T,
{
    use std::{
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
        thread,
        time::Instant,
    };

    let canceled = Arc::new(AtomicBool::new(false));
    let watchdog_flag = Arc::clone(&canceled);
    let name_owned = name.to_string();
    let watchdog = thread::spawn(move || {
        let start = Instant::now();
        loop {
            if watchdog_flag.load(Ordering::SeqCst) {
                return;
            }
            if start.elapsed() >= Duration::from_millis(timeout_ms) {
                eprintln!(
                    "ERROR: smoketest watchdog timeout ({} ms) in {} — force exiting",
                    timeout_ms, name_owned
                );
                process::kill_all();
                exit(2);
            }
            thread::sleep(Duration::from_millis(25));
        }
    });

    let value = panic::catch_unwind(AssertUnwindSafe(f));
    canceled.store(true, Ordering::SeqCst);
    if watchdog.join().is_err() {
        // Watchdog thread panicked; treat it as best-effort cleanup.
    }

    value
        .map_err(|payload| WatchdogAbort::panic_from_message(name, panic_message(payload.as_ref())))
}

/// Verify helper metadata and enforce the shared helper catalog limit.
fn ensure_helper_limit() -> Result<()> {
    let mut helpers = BTreeSet::new();
    for case in CASES {
        for helper in case.helpers {
            if helper.summary.trim().is_empty() {
                return Err(Error::InvalidState(format!(
                    "helper {} is missing documentation",
                    helper.name
                )));
            }
            helpers.insert(helper.name);
        }
    }
    if helpers.len() > 12 {
        return Err(Error::InvalidState(format!(
            "helper API exposes {} functions (limit 12)",
            helpers.len()
        )));
    }
    Ok(())
}

/// Create the root scratch directory for the current smoketest run.
fn create_scratch_root() -> Result<PathBuf> {
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| Error::InvalidState("system time before UNIX_EPOCH".into()))?
        .as_millis();
    let path = PathBuf::from("tmp")
        .join("smoketest-scratch")
        .join(format!("run-{ts}"));
    fs::create_dir_all(&path)?;
    Ok(path)
}

/// Create (or reuse) the scratch directory for a specific case.
fn create_case_dir(root: &Path, index: usize, name: &str) -> Result<PathBuf> {
    let sanitized = name.replace('/', "-");
    let dir = root.join(format!("{:02}-{}", index + 1, sanitized));
    fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// Reset the shared world handle and surface quiescence violations with artifacts referenced.
/// Helper sets used by smoketest cases.
const NO_HELPERS: &[HelperDoc] = &[];

/// Additional watchdog slack for fast cases (milliseconds).
const EXTRA_SHORT: u64 = 2_000;
/// Additional watchdog slack for moderate cases (milliseconds).
const EXTRA_MEDIUM: u64 = 3_000;

/// Registry of retained smoketest cases (window operations removed).
static CASES: &[CaseEntry] = &[
    CaseEntry::main(
        "repeat-relay",
        Some("Measure relay repeat throughput."),
        EXTRA_SHORT,
        Budget::new(200, 1200, 400),
        NO_HELPERS,
        cases::repeat_relay_throughput,
    ),
    CaseEntry::background(
        "repeat-shell",
        Some("Measure shell repeat throughput."),
        EXTRA_SHORT,
        Budget::new(200, 1200, 400),
        NO_HELPERS,
        cases::repeat_shell_throughput,
    ),
    CaseEntry::background(
        "repeat-volume",
        Some("Measure volume repeat throughput with state restoration."),
        EXTRA_MEDIUM,
        Budget::new(200, 2000, 800),
        NO_HELPERS,
        cases::repeat_volume_throughput,
    ),
    CaseEntry::main(
        "ui.demo.standard",
        Some("Launch the full UI with HUD and details panes."),
        EXTRA_MEDIUM,
        Budget::new(800, 2000, 1200),
        NO_HELPERS,
        cases::ui_demo_standard,
    ),
    CaseEntry::main(
        "ui.demo.mini",
        Some("Launch the minimal UI surface."),
        EXTRA_MEDIUM,
        Budget::new(800, 2000, 1200),
        NO_HELPERS,
        cases::ui_demo_mini,
    ),
    CaseEntry::main(
        "ui.display.mapping",
        Some("Verify HUD placement tracks the focused display."),
        EXTRA_MEDIUM,
        Budget::new(800, 2000, 1200),
        NO_HELPERS,
        cases::ui_display_mapping,
    ),
];
