use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    process::exit,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use hotki_world::{WorldHandle, mimic::pump_active_mimics};
use serde::Serialize;
use tracing::info;

use crate::{
    cases,
    error::{Error, Result, print_hints},
    process,
    warn_overlay::OverlaySession,
    world,
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

/// Retrieve a case registry entry by slug or CLI alias.
pub fn case_by_alias(alias: &str) -> Option<&'static CaseEntry> {
    CASES
        .iter()
        .find(|entry| entry.name == alias || entry.aliases.contains(&alias))
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
    /// Alternate CLI aliases that map to this case.
    pub aliases: &'static [&'static str],
    /// Function pointer invoked to execute the case.
    pub run: fn(&mut CaseCtx<'_>) -> Result<()>,
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
    /// Shared world handle used by the case.
    world: WorldHandle,
    /// Scratch directory allocated for the case.
    scratch_dir: PathBuf,
    /// Optional stage timings recorded during execution.
    durations: StageDurationsOptional,
}

impl<'a> CaseCtx<'a> {
    /// Construct a new case context with the provided identifiers.
    pub fn new(name: &'a str, world: WorldHandle, scratch_dir: PathBuf) -> Self {
        Self {
            name,
            world,
            scratch_dir,
            durations: StageDurationsOptional::default(),
        }
    }

    /// Clone the shared world handle for asynchronous operations.
    pub fn world_clone(&self) -> WorldHandle {
        self.world.clone()
    }

    /// Execute the provided stage closure and capture its elapsed duration.
    pub fn stage<F, T>(&mut self, stage: Stage, f: F) -> Result<T>
    where
        F: FnOnce(&mut CaseCtx<'_>) -> Result<T>,
    {
        let start = Instant::now();
        pump_active_mimics();
        let result = f(self);
        pump_active_mimics();
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

    /// Return the case name associated with this context.
    pub fn case_name(&self) -> &str {
        self.name
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
        let world_handle = world::world_handle()?;
        world_handle.reset();
        let outcome = run_entry(entry, config, &scratch_root, idx)?;
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
            let entry = case_by_alias(name)
                .ok_or_else(|| Error::InvalidState(format!("unknown smoketest case: {name}")))?;
            Ok((idx, entry))
        })
        .collect()
}

/// Execute a single registry entry and capture timing/artifact metadata.
fn run_entry(
    entry: &'static CaseEntry,
    config: &RunnerConfig<'_>,
    scratch_root: &Path,
    index: usize,
) -> Result<CaseRunOutcome> {
    let timeout_ms = config
        .base_timeout_ms
        .saturating_add(entry.extra_timeout_ms);

    let case_dir = create_case_dir(scratch_root, index, entry.name)?;
    let start = Instant::now();
    let world = world::world_handle()?;
    let run_case = |world: WorldHandle, scratch_dir: PathBuf| {
        let mut ctx = CaseCtx::new(entry.name, world, scratch_dir);
        let result = (entry.run)(&mut ctx);
        (ctx, result)
    };
    let (ctx, run_result) = if entry.main_thread {
        let world_clone = world.clone();
        let case_dir_clone = case_dir.clone();
        run_on_main_with_watchdog(entry.name, timeout_ms, move || {
            run_case(world_clone, case_dir_clone)
        })
    } else {
        run_with_watchdog(entry.name, timeout_ms, move || run_case(world, case_dir))
    };
    let elapsed = start.elapsed();

    let run_error = run_result.err();
    let world_for_reset = ctx.world_clone();
    let actual_durations = ctx.into_durations();

    log_case_timing(entry, &actual_durations);

    let quiescence_error = ensure_world_quiescent(&world_for_reset, entry.name)?;

    Ok(CaseRunOutcome {
        entry,
        elapsed,
        primary_error: run_error,
        quiescence_error,
    })
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

/// Run `f` on a background thread and bail out if the timeout expires.
fn run_with_watchdog<F, T>(name: &str, timeout_ms: u64, f: F) -> T
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    use std::{sync::mpsc, thread};

    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let value = f();
        if tx.send(value).is_err() {
            // Receiver dropped due to timeout; nothing else to do.
        }
    });

    match rx.recv_timeout(Duration::from_millis(timeout_ms)) {
        Ok(value) => value,
        Err(_) => {
            eprintln!(
                "ERROR: smoketest watchdog timeout ({} ms) in {} — force exiting",
                timeout_ms, name
            );
            process::kill_all();
            exit(2);
        }
    }
}

/// Run `f` on the current thread while a watchdog enforces the deadline.
fn run_on_main_with_watchdog<F, T>(name: &str, timeout_ms: u64, f: F) -> T
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

    let value = f();
    canceled.store(true, Ordering::SeqCst);
    if watchdog.join().is_err() {
        // Watchdog thread panicked; treat it as best-effort cleanup.
    }
    value
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
fn ensure_world_quiescent(world: &WorldHandle, case: &str) -> Result<Option<Error>> {
    let report = world.reset();
    if report.is_quiescent() {
        return Ok(None);
    }
    let msg = format!(
        "{}: world not quiescent after run (ax={}, main_ops={}, mimics={}, subs={})",
        case,
        report.active_ax_observers,
        report.pending_main_ops,
        report.mimic_windows,
        report.subscriptions
    );
    Ok(Some(Error::InvalidState(msg)))
}

/// Helper functions shared by hide and placement-focused smoketest cases.
const PLACE_HELPERS: &[HelperDoc] = &[
    HelperDoc {
        name: "WorldHandle::window_observer",
        summary: "Create per-window observers that block on deterministic frame and mode waits.",
    },
    HelperDoc {
        name: "assert_frame_matches",
        summary: "Emit standardized frame diffs comparing expected geometry with world data.",
    },
];

/// Alias for hide cases since they rely on the same helper set as placement cases.
const HIDE_HELPERS: &[HelperDoc] = PLACE_HELPERS;

/// Helper functions consumed by world-centric smoketests.
const WORLD_HELPERS: &[HelperDoc] = &[
    HelperDoc {
        name: "spawn_scenario",
        summary: "Launch mimic helpers and resolve their world identifiers for assertions.",
    },
    HelperDoc {
        name: "raise_window",
        summary: "Raise helper windows by label using world raise intents.",
    },
];

/// Helper functions consumed by UI demo smoketests.
const UI_HELPERS: &[HelperDoc] = &[HelperDoc {
    name: "HotkiSession::builder",
    summary: "Launch a scoped hotki session backed by a temporary config.",
}];

/// Helper functions consumed by fullscreen smoketests.
const FULLSCREEN_HELPERS: &[HelperDoc] = &[];

/// Registry of Stage Five mimic-driven placement cases.
static CASES: &[CaseEntry] = &[
    CaseEntry {
        name: "repeat-relay",
        info: Some("Relay repeat throughput over the mimic harness"),
        main_thread: true,
        extra_timeout_ms: 5_000,
        budget: Budget {
            setup_ms: 1_200,
            action_ms: 1_600,
            settle_ms: 600,
        },
        helpers: &[],
        aliases: &[],
        run: cases::repeat_relay_throughput,
    },
    CaseEntry {
        name: "repeat-shell",
        info: Some("Shell repeat throughput using the registry runner"),
        main_thread: false,
        extra_timeout_ms: 5_000,
        budget: Budget {
            setup_ms: 1_000,
            action_ms: 1_600,
            settle_ms: 600,
        },
        helpers: &[],
        aliases: &[],
        run: cases::repeat_shell_throughput,
    },
    CaseEntry {
        name: "repeat-volume",
        info: Some("Volume repeat throughput with restore-on-exit"),
        main_thread: false,
        extra_timeout_ms: 6_000,
        budget: Budget {
            setup_ms: 1_000,
            action_ms: 2_600,
            settle_ms: 600,
        },
        helpers: &[],
        aliases: &[],
        run: cases::repeat_volume_throughput,
    },
    CaseEntry {
        name: "raise",
        info: Some("Raise windows by title using world focus APIs"),
        main_thread: true,
        extra_timeout_ms: 10_000,
        budget: Budget {
            setup_ms: 1_200,
            action_ms: 800,
            settle_ms: 1_600,
        },
        helpers: &[],
        aliases: &[],
        run: cases::raise,
    },
    CaseEntry {
        name: "focus.tracking",
        info: Some("Track focus transitions for helper windows"),
        main_thread: true,
        extra_timeout_ms: 60_000,
        budget: Budget {
            setup_ms: 2_000,
            action_ms: 3_000,
            settle_ms: 1_000,
        },
        helpers: &[],
        aliases: &[],
        run: cases::focus_tracking,
    },
    CaseEntry {
        name: "focus.nav",
        info: Some("Navigate focus across helper windows via focus actions"),
        main_thread: true,
        extra_timeout_ms: 60_000,
        budget: Budget {
            setup_ms: 2_000,
            action_ms: 4_000,
            settle_ms: 1_000,
        },
        helpers: &[],
        aliases: &[],
        run: cases::focus_nav,
    },
    CaseEntry {
        name: "hide.toggle.roundtrip",
        info: Some("Toggle hide on/off via world hide intents and verify window restoration"),
        main_thread: true,
        extra_timeout_ms: 20_000,
        budget: Budget {
            setup_ms: 1_200,
            action_ms: 800,
            settle_ms: 1_800,
        },
        helpers: HIDE_HELPERS,
        aliases: &[],
        run: cases::hide_toggle_roundtrip,
    },
    CaseEntry {
        name: "place.fake.adapter",
        info: Some("Exercise fake adapter placement flows without spawning helpers"),
        main_thread: true,
        extra_timeout_ms: 5_000,
        budget: Budget {
            setup_ms: 400,
            action_ms: 200,
            settle_ms: 400,
        },
        helpers: &[],
        aliases: &[],
        run: cases::place_fake_adapter,
    },
    CaseEntry {
        name: "place.minimized.defer",
        info: Some("Auto-unminimize minimized helper window before placement"),
        main_thread: true,
        extra_timeout_ms: 15_000,
        budget: Budget {
            setup_ms: 1_200,
            action_ms: 400,
            settle_ms: 2_000,
        },
        helpers: PLACE_HELPERS,
        aliases: &[],
        run: cases::place_minimized_defer,
    },
    CaseEntry {
        name: "place.zoomed.normalize",
        info: Some("Normalize placement after starting from a zoomed helper window"),
        main_thread: true,
        extra_timeout_ms: 15_000,
        budget: Budget {
            setup_ms: 1_200,
            action_ms: 600,
            settle_ms: 2_400,
        },
        helpers: PLACE_HELPERS,
        aliases: &[],
        run: cases::place_zoomed_normalize,
    },
    CaseEntry {
        name: "place.animated.tween",
        info: Some("Tweened placement verifies animated frame convergence"),
        main_thread: true,
        extra_timeout_ms: 35_000,
        budget: Budget {
            setup_ms: 1_200,
            action_ms: 450,
            settle_ms: 2_400,
        },
        helpers: PLACE_HELPERS,
        aliases: &[],
        run: cases::place_animated_tween,
    },
    CaseEntry {
        name: "place.async.delay",
        info: Some("Delayed apply placement converges after artificial async lag"),
        main_thread: true,
        extra_timeout_ms: 35_000,
        budget: Budget {
            setup_ms: 3_000,
            action_ms: 1_000,
            settle_ms: 2_800,
        },
        helpers: PLACE_HELPERS,
        aliases: &[],
        run: cases::place_async_delay,
    },
    CaseEntry {
        name: "place.term.anchor",
        info: Some("Terminal-style placement honors step-size anchors without post-move drift"),
        main_thread: true,
        extra_timeout_ms: 10_000,
        budget: Budget {
            setup_ms: 1_200,
            action_ms: 500,
            settle_ms: 2_000,
        },
        helpers: PLACE_HELPERS,
        aliases: &[],
        run: cases::place_term_anchor,
    },
    CaseEntry {
        name: "place.increments.anchor",
        info: Some("Placement with resize increments anchors both 2x2 and 3x1 scenarios"),
        main_thread: true,
        extra_timeout_ms: 12_000,
        budget: Budget {
            setup_ms: 1_200,
            action_ms: 800,
            settle_ms: 2_400,
        },
        helpers: PLACE_HELPERS,
        aliases: &[],
        run: cases::place_increments_anchor,
    },
    CaseEntry {
        name: "place.move.min",
        info: Some("Move within grid when minimum height exceeds cell"),
        main_thread: true,
        extra_timeout_ms: 5_000,
        budget: Budget {
            setup_ms: 1_200,
            action_ms: 450,
            settle_ms: 2_400,
        },
        helpers: PLACE_HELPERS,
        aliases: &[],
        run: cases::place_move_min_anchor,
    },
    CaseEntry {
        name: "place.move.nonresizable",
        info: Some("Move anchored fallback when resizing is disabled"),
        main_thread: true,
        extra_timeout_ms: 5_000,
        budget: Budget {
            setup_ms: 1_200,
            action_ms: 450,
            settle_ms: 2_400,
        },
        helpers: PLACE_HELPERS,
        aliases: &[],
        run: cases::place_move_nonresizable_anchor,
    },
    CaseEntry {
        name: "place.grid.cycle",
        info: Some("Cycle helper placement across every grid cell"),
        main_thread: true,
        extra_timeout_ms: 30_000,
        budget: Budget {
            setup_ms: 1_800,
            action_ms: 12_000,
            settle_ms: 3_000,
        },
        helpers: PLACE_HELPERS,
        aliases: &[],
        run: cases::place_grid_cycle,
    },
    CaseEntry {
        name: "place.flex.default",
        info: Some("Flexible placement settles without forcing retries"),
        main_thread: true,
        extra_timeout_ms: 12_000,
        budget: Budget {
            setup_ms: 1_200,
            action_ms: 600,
            settle_ms: 2_400,
        },
        helpers: PLACE_HELPERS,
        aliases: &[],
        run: cases::place_flex_default,
    },
    CaseEntry {
        name: "place.flex.force_size_pos",
        info: Some("Force size->pos retries to confirm opposite ordering attempts"),
        main_thread: true,
        extra_timeout_ms: 12_000,
        budget: Budget {
            setup_ms: 1_200,
            action_ms: 650,
            settle_ms: 2_400,
        },
        helpers: PLACE_HELPERS,
        aliases: &[],
        run: cases::place_flex_force_size_pos,
    },
    CaseEntry {
        name: "place.flex.smg",
        info: Some("Force shrink->move->grow fallback sequencing"),
        main_thread: true,
        extra_timeout_ms: 15_000,
        budget: Budget {
            setup_ms: 1_200,
            action_ms: 700,
            settle_ms: 2_800,
        },
        helpers: PLACE_HELPERS,
        aliases: &[],
        run: cases::place_flex_smg,
    },
    CaseEntry {
        name: "place.skip.nonmovable",
        info: Some("Placement skips non-movable helper windows"),
        main_thread: true,
        extra_timeout_ms: 8_000,
        budget: Budget {
            setup_ms: 1_200,
            action_ms: 700,
            settle_ms: 1_400,
        },
        helpers: PLACE_HELPERS,
        aliases: &[],
        run: cases::place_skip_nonmovable,
    },
    CaseEntry {
        name: "ui.demo.standard",
        info: Some("HUD demo flows through activation, theme cycle, and exit"),
        main_thread: true,
        extra_timeout_ms: 45_000,
        budget: Budget {
            setup_ms: 2_000,
            action_ms: 6_000,
            settle_ms: 2_000,
        },
        helpers: UI_HELPERS,
        aliases: &[],
        run: cases::ui_demo_standard,
    },
    CaseEntry {
        name: "ui.demo.mini",
        info: Some("Mini HUD demo mirrors the standard flow in compact mode"),
        main_thread: true,
        extra_timeout_ms: 45_000,
        budget: Budget {
            setup_ms: 2_000,
            action_ms: 5_000,
            settle_ms: 2_000,
        },
        helpers: UI_HELPERS,
        aliases: &[],
        run: cases::ui_demo_mini,
    },
    CaseEntry {
        name: "fullscreen.toggle.nonnative",
        info: Some("Toggle non-native fullscreen via injected chords and AX validation"),
        main_thread: true,
        extra_timeout_ms: 20_000,
        budget: Budget {
            setup_ms: 1_500,
            action_ms: 3_000,
            settle_ms: 1_500,
        },
        helpers: FULLSCREEN_HELPERS,
        aliases: &[],
        run: cases::fullscreen_toggle_nonnative,
    },
    CaseEntry {
        name: "world.status.permissions",
        info: Some("World status reports granted capabilities and sane polling budgets"),
        main_thread: true,
        extra_timeout_ms: 6_000,
        budget: Budget {
            setup_ms: 900,
            action_ms: 600,
            settle_ms: 900,
        },
        helpers: WORLD_HELPERS,
        aliases: &[],
        run: cases::world_status_permissions,
    },
    CaseEntry {
        name: "world.ax.focus_props",
        info: Some("Focused window exposes AX props through world snapshots"),
        main_thread: true,
        extra_timeout_ms: 8_000,
        budget: Budget {
            setup_ms: 900,
            action_ms: 1_000,
            settle_ms: 900,
        },
        helpers: WORLD_HELPERS,
        aliases: &[],
        run: cases::world_ax_focus_props,
    },
    CaseEntry {
        name: "world.spaces.adoption",
        info: Some("World adopts mock Mission Control spaces within budget"),
        main_thread: false,
        extra_timeout_ms: 6_000,
        budget: Budget {
            setup_ms: 400,
            action_ms: 1_200,
            settle_ms: 400,
        },
        helpers: &[],
        aliases: &[],
        run: cases::world_spaces_adoption,
    },
];
