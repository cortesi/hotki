use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use hotki_world::{WorldHandle, mimic::pump_active_mimics};
use serde::Serialize;

use crate::{
    artifacts, cases,
    error::{Error, Result, print_hints},
    process, world,
};

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

/// Mutable case context that accumulates stage timings and artifacts.
pub struct CaseCtx<'a> {
    /// Registry slug associated with the current case.
    name: &'a str,
    /// Shared world handle used by the case.
    world: WorldHandle,
    /// Artifact directory allocated for the case.
    artifacts_dir: PathBuf,
    /// Optional stage timings recorded during execution.
    durations: StageDurationsOptional,
    /// Artifact paths registered during execution.
    artifacts: Vec<PathBuf>,
}

impl<'a> CaseCtx<'a> {
    /// Construct a new case context with the provided identifiers.
    pub fn new(name: &'a str, world: WorldHandle, artifacts_dir: PathBuf) -> Self {
        Self {
            name,
            world,
            artifacts_dir,
            durations: StageDurationsOptional::default(),
            artifacts: Vec::new(),
        }
    }

    /// Clone the shared world handle for asynchronous operations.
    pub fn world_clone(&self) -> WorldHandle {
        self.world.clone()
    }

    /// Return the artifact directory assigned to the case.
    pub fn artifacts_dir(&self) -> &Path {
        &self.artifacts_dir
    }

    /// Execute the provided stage closure and capture its elapsed duration.
    pub fn stage<F, T>(&mut self, stage: Stage, f: F) -> Result<T>
    where
        F: FnOnce(&mut StageHandle<'_>) -> Result<T>,
    {
        let mut handle = StageHandle {
            name: self.name,
            world: self.world.clone(),
            artifacts_dir: &self.artifacts_dir,
            artifacts: &mut self.artifacts,
        };
        let start = Instant::now();
        pump_active_mimics();
        let result = f(&mut handle);
        pump_active_mimics();
        let elapsed_ms = start.elapsed().as_millis() as u64;
        self.durations.set(stage, elapsed_ms)?;
        result
    }

    /// Consume the context and return recorded stage durations and artifacts.
    pub(crate) fn finish(self) -> CaseReport {
        CaseReport {
            durations: self.durations,
            artifacts: self.artifacts,
        }
    }

    /// Run the setup stage and record the elapsed duration.
    pub fn setup<F, T>(&mut self, f: F) -> Result<T>
    where
        F: FnOnce(&mut StageHandle<'_>) -> Result<T>,
    {
        self.stage(Stage::Setup, f)
    }

    /// Run the action stage and record the elapsed duration.
    pub fn action<F, T>(&mut self, f: F) -> Result<T>
    where
        F: FnOnce(&mut StageHandle<'_>) -> Result<T>,
    {
        self.stage(Stage::Action, f)
    }

    /// Run the settle stage and record the elapsed duration.
    pub fn settle<F, T>(&mut self, f: F) -> Result<T>
    where
        F: FnOnce(&mut StageHandle<'_>) -> Result<T>,
    {
        self.stage(Stage::Settle, f)
    }
}

/// Stage-scoped handle that exposes world accessors and artifact recording.
pub struct StageHandle<'a> {
    /// Case name associated with the current stage.
    name: &'a str,
    /// Shared world handle used to drive requests and fetch frames.
    world: WorldHandle,
    /// Artifact directory allocated for the case.
    artifacts_dir: &'a Path,
    /// Collected artifacts recorded during stage execution.
    artifacts: &'a mut Vec<PathBuf>,
}

impl<'a> StageHandle<'a> {
    /// Clone the shared world handle for asynchronous operations.
    pub fn world_clone(&self) -> WorldHandle {
        self.world.clone()
    }

    /// Return the artifact directory assigned to the case.
    pub fn artifacts_dir(&self) -> &Path {
        self.artifacts_dir
    }

    /// Record an artifact path to be surfaced in case summaries.
    pub fn record_artifact<P: AsRef<Path>>(&mut self, path: P) {
        self.artifacts.push(path.as_ref().to_path_buf());
    }

    /// Return the case name associated with this stage.
    pub fn case_name(&self) -> &str {
        self.name
    }
}

/// Internal representation of stage timings and artifacts produced by a case.
pub struct CaseReport {
    /// Optional stage durations recorded while running the case.
    durations: StageDurationsOptional,
    /// Artifact paths registered during execution.
    artifacts: Vec<PathBuf>,
}

impl CaseReport {
    /// Consume the report and return stage durations plus recorded artifacts.
    fn into_parts(self) -> (StageDurationsOptional, Vec<PathBuf>) {
        (self.durations, self.artifacts)
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
    /// Artifact paths recorded while executing the case.
    pub artifacts: Vec<PathBuf>,
    /// Primary execution error, if any, returned by the case body.
    pub primary_error: Option<Error>,
    /// Quiescence check failure raised during teardown, if any.
    pub quiescence_error: Option<Error>,
}

/// Run every registered case in order, respecting the supplied runner configuration.
pub fn run_all(config: &RunnerConfig<'_>) -> Result<()> {
    ensure_helper_limit()?;
    let artifact_root = create_artifact_root()?;
    let mut failures: Vec<CaseRunOutcome> = Vec::new();
    for (idx, entry) in CASES.iter().enumerate() {
        if !config.quiet {
            crate::heading(&format!("Test: {}", entry.name));
        }
        let world_handle = world::world_handle()?;
        world_handle.reset();
        let outcome = run_entry(entry, config, &artifact_root, idx)?;
        report_outcome(&outcome, config.quiet);
        if outcome.primary_error.is_some() || outcome.quiescence_error.is_some() {
            failures.push(outcome);
            if config.fail_fast {
                break;
            }
        }
    }
    if failures.is_empty() {
        if !config.quiet {
            println!("All smoketest cases passed");
        }
        Ok(())
    } else {
        let names = failures
            .iter()
            .map(|outcome| outcome.entry.name)
            .collect::<Vec<_>>()
            .join(", ");
        Err(Error::InvalidState(format!(
            "smoketest cases failed: {names}"
        )))
    }
}

/// Run a subset of cases by name, preserving the CLI sequence order.
pub fn run_sequence(names: &[&str], config: &RunnerConfig<'_>) -> Result<()> {
    ensure_helper_limit()?;
    let artifact_root = create_artifact_root()?;
    let mut failures = Vec::new();
    for (idx, name) in names.iter().enumerate() {
        let entry = CASES
            .iter()
            .find(|case| case.name == *name)
            .ok_or_else(|| Error::InvalidState(format!("unknown smoketest case: {name}")))?;
        if !config.quiet {
            crate::heading(&format!("Test: {}", entry.name));
        }
        let world_handle = world::world_handle()?;
        world_handle.reset();
        let outcome = run_entry(entry, config, &artifact_root, idx)?;
        report_outcome(&outcome, config.quiet);
        if outcome.primary_error.is_some() || outcome.quiescence_error.is_some() {
            failures.push(outcome);
            if config.fail_fast {
                break;
            }
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        let names = failures
            .iter()
            .map(|outcome| outcome.entry.name)
            .collect::<Vec<_>>()
            .join(", ");
        Err(Error::InvalidState(format!(
            "smoketest cases failed: {names}"
        )))
    }
}

/// Execute a single registry entry and capture timing/artifact metadata.
fn run_entry(
    entry: &'static CaseEntry,
    config: &RunnerConfig<'_>,
    artifact_root: &Path,
    index: usize,
) -> Result<CaseRunOutcome> {
    let timeout_ms = config
        .base_timeout_ms
        .saturating_add(entry.extra_timeout_ms);
    let overlay_info = config.overlay_info.or(entry.info).unwrap_or("");
    let mut overlay = None;
    if config.warn_overlay && !config.quiet {
        overlay = process::start_warn_overlay_with_delay();
        process::write_overlay_status(entry.name);
        process::write_overlay_info(overlay_info);
    }

    let case_dir = create_case_dir(artifact_root, index, entry.name)?;
    let start = Instant::now();
    let world = world::world_handle()?;
    let (ctx, run_result) = if entry.main_thread {
        let world_clone = world;
        let case_dir_clone = case_dir;
        crate::run_on_main_with_watchdog(entry.name, timeout_ms, move || {
            let mut ctx = CaseCtx::new(entry.name, world_clone, case_dir_clone);
            let res = (entry.run)(&mut ctx);
            (ctx, res)
        })
    } else {
        let world_clone = world;
        let case_dir_clone = case_dir;
        crate::run_with_watchdog(entry.name, timeout_ms, move || {
            let mut ctx = CaseCtx::new(entry.name, world_clone, case_dir_clone);
            let res = (entry.run)(&mut ctx);
            (ctx, res)
        })
    };
    let elapsed = start.elapsed();

    if let Some(mut child) = overlay
        && let Err(e) = child.kill_and_wait()
    {
        eprintln!("suite: failed to stop overlay for {}: {}", entry.name, e);
    }

    let run_error = run_result.err();
    let world_for_reset = ctx.world_clone();
    let artifacts_dir_path = ctx.artifacts_dir().to_path_buf();
    let report = ctx.finish().into_parts();
    let (actual_durations, mut artifacts) = report;

    let budget_path = artifacts::write_budget_report(
        entry.name,
        &entry.budget,
        &actual_durations,
        &artifacts_dir_path,
    )?;
    artifacts.push(budget_path);

    let quiescence_error = ensure_world_quiescent(&world_for_reset, entry.name, &artifacts)?;

    Ok(CaseRunOutcome {
        entry,
        elapsed,
        artifacts,
        primary_error: run_error,
        quiescence_error,
    })
}

/// Print a user-friendly summary for the supplied outcome.
fn report_outcome(outcome: &CaseRunOutcome, quiet: bool) {
    let elapsed = outcome.elapsed.as_secs_f64();
    if outcome.primary_error.is_none() && outcome.quiescence_error.is_none() {
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
    if !outcome.artifacts.is_empty() {
        for path in &outcome.artifacts {
            eprintln!("  artifact: {}", path.display());
        }
    }
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

/// Create the root artifact directory for the current smoketest run.
fn create_artifact_root() -> Result<PathBuf> {
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| Error::InvalidState("system time before UNIX_EPOCH".into()))?
        .as_millis();
    let path = PathBuf::from("tmp")
        .join("smoketest-artifacts")
        .join(format!("run-{ts}"));
    fs::create_dir_all(&path)?;
    Ok(path)
}

/// Create (or reuse) the artifact directory for a specific case.
fn create_case_dir(root: &Path, index: usize, name: &str) -> Result<PathBuf> {
    let sanitized = name.replace('/', "-");
    let dir = root.join(format!("{:02}-{}", index + 1, sanitized));
    fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// Reset the shared world handle and surface quiescence violations with artifacts referenced.
fn ensure_world_quiescent(
    world: &WorldHandle,
    case: &str,
    artifacts: &[PathBuf],
) -> Result<Option<Error>> {
    let report = world.reset();
    if report.is_quiescent() {
        return Ok(None);
    }
    let msg = format!(
        "{}: world not quiescent after run (ax={}, main_ops={}, mimics={}, subs={}) artifacts={}",
        case,
        report.active_ax_observers,
        report.pending_main_ops,
        report.mimic_windows,
        report.subscriptions,
        format_artifacts(artifacts)
    );
    Ok(Some(Error::InvalidState(msg)))
}

/// Format artifact paths for inclusion in failure messages.
fn format_artifacts(paths: &[PathBuf]) -> String {
    if paths.is_empty() {
        "-".to_string()
    } else {
        paths
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join(",")
    }
}

/// Helper functions used by placement-focused smoketest cases.
const PLACE_HELPERS: &[HelperDoc] = &[
    HelperDoc {
        name: "wait_for_events_or",
        summary: "Pump main-thread events until world state matches the confirmation closure.",
    },
    HelperDoc {
        name: "assert_frame_matches",
        summary: "Emit standardized frame diffs comparing expected geometry with world data.",
    },
];

/// Registry of Stage Five mimic-driven placement cases.
static CASES: &[CaseEntry] = &[
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
        run: cases::place_minimized_defer,
    },
    CaseEntry {
        name: "place.animated.tween",
        info: Some("Tweened placement verifies animated frame convergence"),
        main_thread: true,
        extra_timeout_ms: 18_000,
        budget: Budget {
            setup_ms: 1_200,
            action_ms: 450,
            settle_ms: 2_400,
        },
        helpers: PLACE_HELPERS,
        run: cases::place_animated_tween,
    },
    CaseEntry {
        name: "place.async.delay",
        info: Some("Delayed apply placement converges after artificial async lag"),
        main_thread: true,
        extra_timeout_ms: 18_000,
        budget: Budget {
            setup_ms: 1_200,
            action_ms: 500,
            settle_ms: 2_800,
        },
        helpers: PLACE_HELPERS,
        run: cases::place_async_delay,
    },
];
