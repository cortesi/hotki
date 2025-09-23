//! Shared helpers for mimic-driven smoketest cases.

use std::{
    cell::Cell,
    collections::HashMap,
    fs,
    future::Future,
    path::PathBuf,
    sync::{Arc, mpsc},
    thread,
    time::{Duration, Instant},
};

use hotki_world::{
    EventCursor, PlaceOptions, RaiseIntent, VisibilityPolicy, WindowKey, WorldEvent, WorldHandle,
    WorldWindow,
    mimic::{
        HelperConfig, MimicHandle, MimicScenario, MimicSpec, Quirk, kill_mimic, pump_active_mimics,
        spawn_mimic,
    },
};
use hotki_world_ids::WorldWindowId;
use mac_winops;
use regex::Regex;
use tracing::{debug, warn};

use crate::{
    error::{Error, Result},
    helpers, runtime,
    suite::StageHandle,
};

thread_local! {
    static DRAIN_MAIN_OPS: Cell<bool> = const { Cell::new(true) };
}

/// Guard that temporarily stops draining main-thread operations while active.
pub struct MainOpsDrainGuard {
    /// Previous drain flag restored when the guard is dropped.
    prev: bool,
}

impl MainOpsDrainGuard {
    /// Disable draining and return a guard that restores the prior state on drop.
    pub fn disable() -> Self {
        let prev = DRAIN_MAIN_OPS.with(|cell| {
            let prev = cell.get();
            cell.set(false);
            prev
        });
        Self { prev }
    }
}

impl Drop for MainOpsDrainGuard {
    fn drop(&mut self) {
        DRAIN_MAIN_OPS.with(|cell| cell.set(self.prev));
    }
}

/// Initial number of mimic pump iterations to let helper windows settle.
const INITIAL_SPIN_ITERS: usize = 24;

/// Maximum number of events drained while priming a fresh cursor.
const DRAIN_LIMIT: u32 = 5_000;

/// Run `fut` on the shared runtime while continuing to pump mimic event loops.
pub fn block_on_with_pump<F>(fut: F) -> Result<F::Output>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    let (tx, rx) = mpsc::channel::<Result<F::Output>>();
    thread::spawn(move || {
        let result = runtime::block_on(fut);
        if let Err(err) = tx.send(result) {
            warn!(?err, "block_on_with_pump: receiver dropped runtime result");
        }
    });
    loop {
        if DRAIN_MAIN_OPS.with(|cell| cell.get()) {
            mac_winops::drain_main_ops();
        }
        pump_active_mimics();
        let pump_deadline = Instant::now() + Duration::from_millis(5);
        mac_winops::wait_main_ops_idle(pump_deadline);
        match rx.try_recv() {
            Ok(result) => return result,
            Err(mpsc::TryRecvError::Disconnected) => {
                return Err(Error::InvalidState("async task dropped".into()));
            }
            Err(mpsc::TryRecvError::Empty) => thread::sleep(Duration::from_millis(1)),
        }
    }
}

/// Specification for a window that should be spawned within a mimic scenario.
pub struct WindowSpawnSpec {
    /// Label used to reference the helper window within a scenario.
    pub label: &'static str,
    /// Title assigned to the helper window when spawned.
    pub title: &'static str,
    /// Placement options supplied to the helper window.
    pub place: PlaceOptions,
    /// Mimic quirks enabled for the helper window.
    pub quirks: Vec<Quirk>,
    /// Callback invoked to tweak helper configuration prior to launch.
    pub configure: Box<dyn FnOnce(&mut HelperConfig) + Send + 'static>,
}

impl WindowSpawnSpec {
    /// Create a new window specification with default placement options.
    pub fn new(label: &'static str, title: &'static str) -> Self {
        Self {
            label,
            title,
            place: PlaceOptions::default(),
            quirks: Vec::new(),
            configure: Box::new(|_| {}),
        }
    }

    /// Provide a configuration callback that can tweak the helper runtime.
    #[must_use]
    pub fn configure(mut self, f: impl FnOnce(&mut HelperConfig) + Send + 'static) -> Self {
        self.configure = Box::new(f);
        self
    }
}

/// Metadata for a spawned mimic window tracked during a scenario.
pub struct ScenarioWindow {
    /// Title assigned to the helper window.
    pub(crate) title: String,
    /// Composite world identifier for the helper window.
    pub(crate) world_id: WorldWindowId,
    /// Window key used when querying world APIs.
    pub(crate) key: WindowKey,
}

impl ScenarioWindow {}

/// State captured after spawning a mimic scenario.
pub struct ScenarioState {
    /// Scenario slug recorded across artifacts and helper labels.
    pub(crate) slug: &'static str,
    /// Handle to the active mimic scenario.
    pub(crate) mimic: MimicHandle,
    /// Event cursor positioned after the initial snapshot.
    pub(crate) cursor: EventCursor,
    /// Mapping from window label to observed metadata.
    pub(crate) windows: HashMap<&'static str, ScenarioWindow>,
}

impl ScenarioState {
    /// Retrieve metadata for a window identified by label.
    pub(crate) fn window(&self, label: &str) -> Result<&ScenarioWindow> {
        self.windows
            .get(label)
            .ok_or_else(|| Error::InvalidState(format!("unknown window label: {label}")))
    }
}

/// Internal helper describing a window marker expected in the snapshot.
struct WindowDescriptor {
    /// Window label exposed by the scenario.
    label: &'static str,
    /// Title marker embedded in helper window titles.
    marker: String,
}

/// Spawn a mimic scenario and wait for declared windows to surface in the world snapshot.
pub fn spawn_scenario(
    stage: &StageHandle<'_>,
    slug: &'static str,
    specs: Vec<WindowSpawnSpec>,
) -> Result<ScenarioState> {
    let total_start = Instant::now();
    let world = stage.world_clone();
    let mut descriptors = Vec::with_capacity(specs.len());
    let mut mimic_specs = Vec::with_capacity(specs.len());
    for spec in specs {
        let slug_arc: Arc<str> = Arc::from(slug);
        let label_arc: Arc<str> = Arc::from(spec.label);
        let marker = format!("[{slug}::{}]", spec.label);
        let mut config = HelperConfig {
            scenario_slug: slug_arc.clone(),
            window_label: label_arc.clone(),
            ..HelperConfig::default()
        };
        (spec.configure)(&mut config);

        let mimic_spec = MimicSpec::new(slug_arc.clone(), label_arc.clone(), spec.title)
            .with_place(spec.place)
            .with_quirks(spec.quirks.clone())
            .with_config(config);
        mimic_specs.push(mimic_spec);
        descriptors.push(WindowDescriptor {
            label: spec.label,
            marker,
        });
    }

    let spawn_start = Instant::now();
    let scenario = MimicScenario::new(Arc::from(slug), mimic_specs);
    let mimic = spawn_mimic_handle(slug, scenario)?;
    let _spawn_ms = spawn_start.elapsed().as_millis();
    pump_active_mimics();

    let subscribe_start = Instant::now();
    let world_for_subscribe = world.clone();
    let (mut cursor, mut snapshot, _) =
        block_on_with_pump(async move { world_for_subscribe.subscribe_with_snapshot().await })?;
    let _subscribe_ms = subscribe_start.elapsed().as_millis();

    let mut windows: HashMap<&'static str, ScenarioWindow> = HashMap::new();
    let resolve_start = Instant::now();
    let mut attempts = 0u32;
    while windows.len() < descriptors.len() {
        attempt_resolve_windows(&world, &descriptors, &snapshot, &mut windows)?;
        if windows.len() == descriptors.len() {
            break;
        }
        if attempts >= 40 {
            return Err(Error::InvalidState(format!(
                "mimic scenario '{slug}' did not expose all windows"
            )));
        }
        attempts += 1;
        pump_active_mimics();
        thread::sleep(Duration::from_millis(5));
        let world_for_snapshot = world.clone();
        snapshot = block_on_with_pump(async move { world_for_snapshot.snapshot().await })?;
    }
    let _resolve_ms = resolve_start.elapsed().as_millis();

    // Allow mimic timers to run before issuing actions.
    let pre_spin_start = Instant::now();
    for _ in 0..INITIAL_SPIN_ITERS {
        pump_active_mimics();
        thread::sleep(Duration::from_millis(5));
    }
    let _spin_ms = pre_spin_start.elapsed().as_millis();
    let _settle_ms = total_start.elapsed().as_millis();

    // Drain any queued events so we start the case with a quiet cursor.
    // Cap draining so continual event streams do not stall case setup.
    let mut drained_events: u32 = 0;
    let drain_deadline = Instant::now() + Duration::from_millis(750);
    loop {
        let mut drained_this_round = 0;
        while world.next_event_now(&mut cursor).is_some() {
            drained_events = drained_events.saturating_add(1);
            drained_this_round += 1;
            if drained_events >= DRAIN_LIMIT {
                break;
            }
        }
        if drained_this_round == 0 || drained_events >= DRAIN_LIMIT {
            break;
        }
        if Instant::now() >= drain_deadline {
            debug!(slug, drained_events, "spawn_scenario_drain_capped");
            break;
        }
        pump_active_mimics();
        thread::sleep(Duration::from_millis(5));
    }
    pump_active_mimics();
    debug!(slug, drained_events, "spawn_scenario_drain_done");

    Ok(ScenarioState {
        slug,
        mimic,
        cursor,
        windows,
    })
}

/// Spawn the supplied mimic scenario, mapping any failure into a smoketest error.
fn spawn_mimic_handle(slug: &str, scenario: MimicScenario) -> Result<MimicHandle> {
    debug!(slug, "spawn_mimic_handle_start");
    let handle = spawn_mimic(scenario)
        .map_err(|e| Error::InvalidState(format!("spawn mimic failed for {}: {}", slug, e)))?;
    debug!(slug, "spawn_mimic_handle_done");
    Ok(handle)
}

/// Attempt to resolve helper windows within the current world snapshot.
fn attempt_resolve_windows(
    world: &WorldHandle,
    descriptors: &[WindowDescriptor],
    snapshot: &[WorldWindow],
    windows: &mut HashMap<&'static str, ScenarioWindow>,
) -> Result<()> {
    for desc in descriptors {
        if windows.contains_key(desc.label) {
            continue;
        }
        for win in snapshot
            .iter()
            .filter(|win| win.title.contains(&desc.marker))
        {
            debug!(
                label = desc.label,
                pid = win.pid,
                id = win.id,
                layer = win.layer,
                focused = win.focused,
                on_screen = win.is_on_screen,
                z = win.z,
                "scenario_window_candidate"
            );
        }
        if let Some(candidate) = snapshot
            .iter()
            .filter(|win| win.title.contains(&desc.marker))
            .min_by_key(|win| (win.layer != 0, !win.is_on_screen, win.z))
        {
            let mut window = candidate.clone();
            window = ensure_window_ready(world, &desc.marker, window)?;
            if let Err(err) = mac_winops::request_activate_pid(window.pid) {
                debug!(pid = window.pid, ?err, "activate pid request failed");
            }
            windows.insert(
                desc.label,
                ScenarioWindow {
                    title: window.title.clone(),
                    world_id: WorldWindowId::new(window.pid, window.id),
                    key: WindowKey {
                        pid: window.pid,
                        id: window.id,
                    },
                },
            );
        }
    }
    Ok(())
}

/// Ensure the helper window is visible before returning its metadata.
pub fn ensure_window_ready(
    world: &WorldHandle,
    marker: &str,
    window: WorldWindow,
) -> Result<WorldWindow> {
    if window.is_on_screen {
        return Ok(window);
    }
    let key = WindowKey {
        pid: window.pid,
        id: window.id,
    };
    let config = helpers::default_wait_config();
    let wait_result = block_on_with_pump({
        let world = world.clone();
        async move {
            let mut observer = world.window_observer_with_config(key, config);
            observer
                .wait_for_visibility(VisibilityPolicy::OnScreen)
                .await
        }
    });
    match wait_result {
        Ok(Ok(updated)) => Ok(updated),
        Ok(Err(err)) => {
            debug!(marker, error = %err, "ensure_window_ready_wait_failed");
            Ok(window)
        }
        Err(err) => {
            debug!(marker, error = %err, "ensure_window_ready_runtime_failed");
            Ok(window)
        }
    }
}

/// Record mimic diagnostics to the artifact directory and return the recorded path.
pub fn record_mimic_diagnostics(
    stage: &mut StageHandle<'_>,
    slug: &str,
    mimic: &MimicHandle,
) -> Result<PathBuf> {
    let sanitized = slug.replace('.', "_");
    let diag_path = stage
        .artifacts_dir()
        .join(format!("{}_mimic.txt", sanitized));
    fs::write(&diag_path, mimic.diagnostics().join("\n"))?;
    stage.record_artifact(&diag_path);
    Ok(diag_path)
}

/// Kill the supplied mimic handle, converting errors into smoketest failures.
pub fn shutdown_mimic(handle: MimicHandle) -> Result<()> {
    kill_mimic(handle).map_err(|e| Error::InvalidState(format!("mimic shutdown failed: {e}")))
}

/// Raise a helper window identified by `label`, asserting focus updates.
pub fn raise_window(stage: &StageHandle<'_>, state: &mut ScenarioState, label: &str) -> Result<()> {
    let start_all = Instant::now();
    let window = state.window(label)?;
    let world = stage.world_clone();
    let window_title = window.title.clone();
    let expected_id = window.world_id;
    let pattern = format!("^{}$", regex::escape(&window_title));
    let regex = Regex::new(&pattern)
        .map_err(|err| Error::InvalidState(format!("invalid raise regex: {err}")))?;
    let intent = RaiseIntent {
        app_regex: None,
        title_regex: Some(Arc::new(regex)),
    };

    let request_world = world.clone();
    let request_start = Instant::now();
    let receipt = block_on_with_pump(async move { request_world.request_raise(intent).await })?
        .map_err(|err| Error::InvalidState(format!("raise request failed: {err}")))?;
    let request_ms = request_start.elapsed().as_millis();

    let target_id = receipt.target_id().ok_or_else(|| {
        Error::InvalidState(format!(
            "raise did not select a target window (label={label} title={window_title})"
        ))
    })?;

    if target_id != expected_id {
        return Err(Error::InvalidState(format!(
            "raise targeted unexpected window: expected pid={} id={} got pid={} id={}",
            expected_id.pid(),
            expected_id.window_id(),
            target_id.pid(),
            target_id.window_id()
        )));
    }

    let wait_start = Instant::now();
    wait_for_focus(stage, state, &world, expected_id)?;
    let wait_ms = wait_start.elapsed().as_millis();
    let total_ms = start_all.elapsed().as_millis();
    debug!(
        case = %stage.case_name(),
        label,
        request_ms,
        wait_ms,
        total_ms,
        "raise_window_timing"
    );
    Ok(())
}

/// Wait until the world reports focus on the expected helper window.
fn wait_for_focus(
    stage: &StageHandle<'_>,
    state: &mut ScenarioState,
    world: &WorldHandle,
    expected_id: WorldWindowId,
) -> Result<()> {
    let expected_pid = expected_id.pid();
    let expected_window = expected_id.window_id();
    let mut logged_none = false;
    let mut logged_mismatch = false;
    let baseline_lost = state.cursor.lost_count;

    let initial_world = world.clone();
    let initial_focus = block_on_with_pump(async move { initial_world.focused().await })?;
    if let Some(key) = initial_focus {
        if key.pid == expected_pid && key.id == expected_window {
            return Ok(());
        }
        debug!(
            case = %stage.case_name(),
            expected_pid,
            expected_window,
            observed_pid = key.pid,
            observed_window = key.id,
            "wait_for_focus_initial_mismatch"
        );
        logged_mismatch = true;
    } else {
        debug!(
            case = %stage.case_name(),
            expected_pid,
            expected_window,
            "wait_for_focus_initial_none"
        );
        logged_none = true;
    }

    let deadline = Instant::now() + Duration::from_millis(10_000);
    loop {
        pump_active_mimics();
        let focus_world = world.clone();
        if let Some(key) = block_on_with_pump(async move { focus_world.focused().await })? {
            if key.pid == expected_pid && key.id == expected_window {
                return Ok(());
            }
            if !logged_mismatch {
                debug!(
                    case = %stage.case_name(),
                    expected_pid,
                    expected_window,
                    observed_pid = key.pid,
                    observed_window = key.id,
                    "wait_for_focus_poll_mismatch"
                );
                logged_mismatch = true;
            }
        } else if !logged_none {
            debug!(
                case = %stage.case_name(),
                expected_pid,
                expected_window,
                "wait_for_focus_poll_none"
            );
            logged_none = true;
        }
        if Instant::now() >= deadline {
            return Err(Error::InvalidState(format!(
                "timeout waiting for {} (lost_count={} next_index={})",
                stage.case_name(),
                state.cursor.lost_count,
                state.cursor.next_index
            )));
        }
        let pump_until = Instant::now() + Duration::from_millis(10);
        world.pump_main_until(pump_until);
        pump_active_mimics();
        while let Some(event) = world.next_event_now(&mut state.cursor) {
            if state.cursor.lost_count > baseline_lost {
                return Err(Error::InvalidState(format!(
                    "events lost during wait (lost_count={}): see artifacts",
                    state.cursor.lost_count
                )));
            }
            if let WorldEvent::FocusChanged(change) = event {
                if let Some(ref key) = change.key {
                    debug!(
                        case = %stage.case_name(),
                        expected_pid,
                        expected_window,
                        observed_pid = key.pid,
                        observed_window = key.id,
                        "focus_event"
                    );
                    if key.pid == expected_pid && key.id == expected_window {
                        return Ok(());
                    }
                    if !logged_mismatch {
                        debug!(
                            case = %stage.case_name(),
                            expected_pid,
                            expected_window,
                            observed_pid = key.pid,
                            observed_window = key.id,
                            "wait_for_focus_mismatch"
                        );
                        logged_mismatch = true;
                    }
                } else {
                    debug!(
                        case = %stage.case_name(),
                        expected_pid,
                        expected_window,
                        "focus_event_none"
                    );
                    if !logged_none {
                        logged_none = true;
                    }
                }
            }
            pump_active_mimics();
        }
        if state.cursor.lost_count > baseline_lost {
            return Err(Error::InvalidState(format!(
                "events lost during wait (lost_count={}): see artifacts",
                state.cursor.lost_count
            )));
        }
    }
}
