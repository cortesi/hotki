//! Shared helpers for mimic-driven smoketest cases.

use std::{
    collections::HashMap,
    fs,
    future::Future,
    path::PathBuf,
    sync::{Arc, mpsc},
    thread,
    time::{Duration, Instant},
};

use hotki_world::{
    EventCursor, PlaceOptions, WorldHandle, WorldWindow,
    mimic::{
        HelperConfig, MimicHandle, MimicScenario, MimicSpec, Quirk, kill_mimic, pump_active_mimics,
        spawn_mimic,
    },
};
use hotki_world_ids::WorldWindowId;
use tracing::{debug, warn};

use crate::{
    error::{Error, Result},
    runtime,
    suite::StageHandle,
};

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
        pump_active_mimics();
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
                },
            );
        }
    }
    Ok(())
}

/// Ensure the helper window is visible before returning its metadata.
fn ensure_window_ready(
    world: &WorldHandle,
    marker: &str,
    mut window: WorldWindow,
) -> Result<WorldWindow> {
    if window.is_on_screen {
        return Ok(window);
    }
    let deadline = Instant::now() + Duration::from_millis(750);
    while !window.is_on_screen && Instant::now() < deadline {
        pump_active_mimics();
        thread::sleep(Duration::from_millis(10));
        let world_for_snapshot = world.clone();
        let refreshed = block_on_with_pump(async move { world_for_snapshot.snapshot().await })?;
        if let Some(updated) = refreshed
            .into_iter()
            .find(|candidate| candidate.title.contains(marker))
        {
            window = updated;
            if window.is_on_screen {
                break;
            }
        }
    }
    Ok(window)
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
