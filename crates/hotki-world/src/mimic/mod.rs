//! Helper window (winit) used by smoketests to verify placement behaviors.

#![allow(clippy::module_name_repetitions)]

use std::{
    fmt,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
};

use once_cell::sync::OnceCell;
use tracing::{debug, warn};

use crate::{PlaceOptions, RaiseStrategy};

/// Internal registry mapping spawned mimic windows to diagnostic metadata so world
/// reconciliation can surface `{scenario_slug, window_label, quirks[]}`.
static REGISTRY: OnceCell<Mutex<MimicRegistry>> = OnceCell::new();

fn registry() -> &'static Mutex<MimicRegistry> {
    REGISTRY.get_or_init(|| Mutex::new(MimicRegistry::default()))
}

fn format_quirks(quirks: &[Quirk]) -> String {
    if quirks.is_empty() {
        "-".to_string()
    } else {
        quirks
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(",")
    }
}

fn parse_decorated_label(title: &str) -> Option<&str> {
    let start = title.rfind("::")?;
    let end = title.rfind(']')?;
    if end <= start + 2 {
        return None;
    }
    Some(&title[start + 2..end])
}

fn should_skip_apply_for_minimized(quirks: &[Quirk], minimized: bool) -> bool {
    minimized
        && quirks
            .iter()
            .any(|q| matches!(q, Quirk::IgnoreMoveIfMinimized))
}

fn select_sibling_for_cycle<'a>(
    windows: &'a [mac_winops::WindowInfo],
    pid: i32,
    current_title: &str,
    slug_fragment: &str,
) -> Option<&'a mac_winops::WindowInfo> {
    windows
        .iter()
        .find(|w| w.pid == pid && w.title != current_title && w.title.contains(slug_fragment))
}

/// Quirks that can be applied to a mimic window to simulate application-specific
/// behaviour. Semantics are intentionally precise so tests can assert outcomes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Quirk {
    /// Round raw Accessibility geometry while leaving CoreGraphics untouched.
    ///
    /// The helper reports CG frames exactly as requested but perturbs AX frames
    /// by rounding towards zero, mimicking real-world apps that quantise AX
    /// positions independently of CG.
    AxRounding,
    /// Delay authoritative geometry updates until the helper runloop has pumped
    /// at least once after observing a `set_frame` request. Mirrors apps that
    /// apply their own debounced move logic.
    DelayApplyMove,
    /// Ignore direct move requests while the window is minimised. Placement
    /// commands must restore the window first; otherwise the helper defers to
    /// preserve the minimised state.
    IgnoreMoveIfMinimized,
    /// Cycle focus between siblings before yielding the target when the runner
    /// requests `RaiseStrategy::KeepFrontWindow`, ensuring the previously front
    /// window remains ahead of the mimic under test.
    RaiseCyclesToSibling,
}

impl fmt::Display for Quirk {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Self::AxRounding => "AxRounding",
            Self::DelayApplyMove => "DelayApplyMove",
            Self::IgnoreMoveIfMinimized => "IgnoreMoveIfMinimized",
            Self::RaiseCyclesToSibling => "RaiseCyclesToSibling",
        };
        write!(f, "{}", name)
    }
}

/// Specification for a single mimic window within a scenario.
#[derive(Clone, Debug)]
pub struct MimicSpec {
    /// Stable slug identifying the scenario that owns this window.
    pub scenario_slug: Arc<str>,
    /// Short label (e.g. `primary`, `sibling`) used in artifacts and diagnostics.
    pub window_label: Arc<str>,
    /// NSWindow title assigned to the helper window.
    pub title: String,
    /// Placement tuning options supplied by smoketest scenarios.
    pub place: PlaceOptions,
    /// Quirk list applied to this helper.
    pub quirks: Vec<Quirk>,
    /// Runtime configuration that mirrors the legacy winhelper knobs.
    pub config: HelperConfig,
}

impl MimicSpec {
    /// Convenience for building a spec with default helper configuration.
    #[must_use]
    pub fn new(slug: Arc<str>, label: impl Into<Arc<str>>, title: impl Into<String>) -> Self {
        Self {
            scenario_slug: slug,
            window_label: label.into(),
            title: title.into(),
            place: PlaceOptions::default(),
            quirks: Vec::new(),
            config: HelperConfig::default(),
        }
    }

    /// Attach quirks to the spec.
    #[must_use]
    pub fn with_quirks(mut self, quirks: Vec<Quirk>) -> Self {
        self.quirks = quirks;
        self
    }

    /// Override placement options.
    #[must_use]
    pub fn with_place(mut self, place: PlaceOptions) -> Self {
        self.place = place;
        self
    }

    /// Override helper configuration.
    #[must_use]
    pub fn with_config(mut self, config: HelperConfig) -> Self {
        self.config = config;
        self
    }
}

/// Scenario container: a slug plus one or more mimic specs.
#[derive(Clone, Debug)]
pub struct MimicScenario {
    /// Stable slug for artifact tagging.
    pub slug: Arc<str>,
    /// Ordered list of mimic windows.
    pub windows: Vec<MimicSpec>,
}

impl MimicScenario {
    /// Construct a scenario with the provided slug and window specifications.
    #[must_use]
    pub fn new(slug: impl Into<Arc<str>>, windows: Vec<MimicSpec>) -> Self {
        Self {
            slug: slug.into(),
            windows,
        }
    }
}

/// Handle returned by [`spawn_mimic`] that manages helper lifetimes.
pub struct MimicHandle {
    slug: Arc<str>,
    windows: Vec<MimicWindowHandle>,
}

impl MimicHandle {
    /// Scenario slug.
    #[must_use]
    pub fn slug(&self) -> &Arc<str> {
        &self.slug
    }

    /// Snapshot diagnostic rows for each helper window.
    #[must_use]
    pub fn diagnostics(&self) -> Vec<String> {
        self.windows
            .iter()
            .map(|w| {
                format!(
                    "{}/{} quirks=[{}] raise={:?} minimized={:?} animate={}",
                    self.slug.as_ref(),
                    w.label.as_ref(),
                    format_quirks(&w.quirks),
                    w.place.raise,
                    w.place.minimized,
                    w.place.animate,
                )
            })
            .collect()
    }
}

struct MimicWindowHandle {
    label: Arc<str>,
    quirks: Vec<Quirk>,
    place: PlaceOptions,
    shutdown: Arc<AtomicBool>,
    join: Mutex<Option<JoinHandle<Result<(), String>>>>,
}

impl MimicWindowHandle {
    fn shutdown(&self) {
        self.shutdown.store(true, Ordering::SeqCst);
    }

    fn join(&self) -> Result<(), MimicError> {
        if let Some(handle) = self.join.lock().unwrap().take() {
            handle
                .join()
                .map_err(|_| MimicError::JoinPanic(self.label.to_string()))?
                .map_err(|e| MimicError::HelperFailure(self.label.to_string(), e))?
        }
        Ok(())
    }
}

/// Errors surfaced by mimic helper management.
#[derive(Debug)]
pub enum MimicError {
    /// Helper thread panicked while exiting.
    JoinPanic(String),
    /// Helper reported a recoverable failure.
    HelperFailure(String, String),
    /// Failed to spawn the helper thread.
    SpawnFailed(String),
}

impl fmt::Display for MimicError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::JoinPanic(label) => write!(f, "mimic window '{}' panicked", label),
            Self::HelperFailure(label, err) => {
                write!(f, "mimic window '{}' failed: {}", label, err)
            }
            Self::SpawnFailed(label) => write!(
                f,
                "failed to spawn mimic window '{}': thread start error",
                label
            ),
        }
    }
}

impl std::error::Error for MimicError {}

/// Launch the provided mimic scenario, returning a handle suitable for teardown.
pub fn spawn_mimic(scenario: MimicScenario) -> Result<MimicHandle, MimicError> {
    if scenario.windows.is_empty() {
        warn!(slug = %scenario.slug, "spawn_mimic called with no windows");
    }
    let slug = scenario.slug.clone();
    let mut handles = Vec::with_capacity(scenario.windows.len());
    for spec in scenario.windows {
        let shutdown = Arc::new(AtomicBool::new(false));
        let label = spec.window_label.clone();
        let quirks = spec.quirks.clone();
        let place = spec.place;
        let mut helper_config = spec.config.clone();
        apply_quirk_defaults(&mut helper_config, &quirks);
        helper_config.place = place;
        helper_config.scenario_slug = spec.scenario_slug.clone();
        helper_config.window_label = spec.window_label.clone();
        let helper_config = helper_config
            .with_shutdown(shutdown.clone())
            .with_quirks(quirks.clone());
        let thread_name = format!("mimic-{}-{}", slug, label);
        let decorated_title = format!(
            "{} [{}::{}]",
            spec.title, spec.scenario_slug, spec.window_label
        );
        debug!(
            tag = %format!(
                "{}/{} quirks=[{}]",
                spec.scenario_slug.as_ref(),
                spec.window_label.as_ref(),
                format_quirks(&quirks)
            ),
            raise = ?place.raise,
            minimized = ?place.minimized,
            animate = place.animate,
            "spawning mimic window"
        );
        let join = thread::Builder::new()
            .name(thread_name.clone())
            .spawn(move || run_helper_window(decorated_title, helper_config))
            .map_err(|_| MimicError::SpawnFailed(thread_name.clone()))?;

        registry()
            .lock()
            .unwrap()
            .register(slug.clone(), label.clone(), quirks.clone(), place);

        handles.push(MimicWindowHandle {
            label,
            quirks,
            place,
            shutdown,
            join: Mutex::new(Some(join)),
        });
    }

    Ok(MimicHandle {
        slug,
        windows: handles,
    })
}

/// Signal and join all helpers for the provided handle.
pub fn kill_mimic(handle: MimicHandle) -> Result<(), MimicError> {
    for window in &handle.windows {
        window.shutdown();
    }
    for window in &handle.windows {
        window.join()?;
    }
    registry().lock().unwrap().purge_slug(&handle.slug);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delay_apply_move_sets_default_delay() {
        let mut cfg = HelperConfig {
            delay_apply_ms: 0,
            ..HelperConfig::default()
        };
        apply_quirk_defaults(&mut cfg, &[Quirk::DelayApplyMove]);
        assert!(cfg.delay_apply_ms > 0);
    }

    #[test]
    fn ax_rounding_sets_step_size() {
        let mut cfg = HelperConfig {
            step_size: None,
            ..HelperConfig::default()
        };
        apply_quirk_defaults(&mut cfg, &[Quirk::AxRounding]);
        assert_eq!(cfg.step_size, Some((1.0, 1.0)));
    }

    #[test]
    fn registry_snapshot_reports_registration() {
        let slug: Arc<str> = Arc::from("test-scenario");
        let label: Arc<str> = Arc::from("primary");
        registry().lock().unwrap().register(
            slug.clone(),
            label.clone(),
            vec![Quirk::DelayApplyMove],
            PlaceOptions::default(),
        );
        let snapshot = registry_snapshot();
        assert!(
            snapshot
                .iter()
                .any(|entry| entry.scenario_slug == slug && entry.window_label == label)
        );
        registry().lock().unwrap().purge_slug(&slug);
    }

    #[test]
    fn select_sibling_finds_matching_window() {
        let slug_fragment = "[demo::";
        let windows = vec![
            test_window(10, "hotki helper [demo::primary]"),
            test_window(10, "hotki helper [demo::sibling]"),
        ];
        let sibling =
            select_sibling_for_cycle(&windows, 10, "hotki helper [demo::primary]", slug_fragment)
                .expect("sibling window");
        assert_eq!(sibling.title, "hotki helper [demo::sibling]");
    }

    #[test]
    fn select_sibling_skips_non_matching_pid() {
        let slug_fragment = "[demo::";
        let windows = vec![
            test_window(11, "hotki helper [demo::primary]"),
            test_window(10, "hotki helper [other::sibling]"),
        ];
        assert!(
            select_sibling_for_cycle(&windows, 10, "hotki helper [demo::primary]", slug_fragment)
                .is_none()
        );
    }

    #[test]
    fn skip_apply_helper_respects_quirk_and_minimize_state() {
        assert!(should_skip_apply_for_minimized(
            &[Quirk::IgnoreMoveIfMinimized],
            true
        ));
        assert!(!should_skip_apply_for_minimized(
            &[Quirk::IgnoreMoveIfMinimized],
            false
        ));
        assert!(!should_skip_apply_for_minimized(&[Quirk::AxRounding], true));
    }

    fn test_window(pid: i32, title: &str) -> mac_winops::WindowInfo {
        mac_winops::WindowInfo {
            app: "TestApp".into(),
            title: title.into(),
            pid,
            id: 42,
            pos: None,
            space: None,
            layer: 0,
            focused: false,
            is_on_screen: true,
            on_active_space: true,
        }
    }
}

/// Runtime helper that owns the registry contents used for diagnostics.
#[derive(Default)]
struct MimicRegistry {
    entries: Vec<MimicRegistryEntry>,
}

impl MimicRegistry {
    fn register(
        &mut self,
        slug: Arc<str>,
        label: Arc<str>,
        quirks: Vec<Quirk>,
        place: PlaceOptions,
    ) {
        self.entries.push(MimicRegistryEntry {
            slug,
            label,
            quirks,
            place,
        });
    }

    fn purge_slug(&mut self, slug: &Arc<str>) {
        self.entries.retain(|entry| &entry.slug != slug);
    }

    #[allow(dead_code)]
    fn snapshot(&self) -> Vec<MimicRegistryEntry> {
        self.entries.clone()
    }
}

#[derive(Clone, Debug)]
struct MimicRegistryEntry {
    slug: Arc<str>,
    label: Arc<str>,
    quirks: Vec<Quirk>,
    place: PlaceOptions,
}

/// Diagnostic snapshot describing an active mimic window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MimicDiagnostic {
    /// Scenario slug owning the window.
    pub scenario_slug: Arc<str>,
    /// Window label recorded for artifacts.
    pub window_label: Arc<str>,
    /// Quirks currently active for the helper.
    pub quirks: Vec<Quirk>,
    /// Placement strategy in effect for the helper window.
    pub place: PlaceOptions,
}

impl MimicDiagnostic {
    /// Return the `{scenario_slug}/{window_label}` identifier for diagnostics.
    #[must_use]
    pub fn tag(&self) -> String {
        format!(
            "{}/{}",
            self.scenario_slug.as_ref(),
            self.window_label.as_ref()
        )
    }

    /// Produce a human-readable quirk list for logging.
    #[must_use]
    pub fn quirks_display(&self) -> String {
        format_quirks(&self.quirks)
    }
}

/// Snapshot the active mimic registry for artifact generation.
#[must_use]
pub fn registry_snapshot() -> Vec<MimicDiagnostic> {
    registry()
        .lock()
        .unwrap()
        .snapshot()
        .into_iter()
        .map(|entry| MimicDiagnostic {
            scenario_slug: entry.slug,
            window_label: entry.label,
            quirks: entry.quirks,
            place: entry.place,
        })
        .collect()
}

/// Configuration knobs for the helper window runtime.
#[derive(Clone, Debug)]
pub struct HelperConfig {
    /// Lifetime for the helper window before automatic shutdown (ms).
    pub time_ms: u64,
    /// Delay applied when the system attempts to set the frame directly (ms).
    pub delay_setframe_ms: u64,
    /// Delay before applying the target frame (ms).
    pub delay_apply_ms: u64,
    /// Duration for tweened placement animations (ms).
    pub tween_ms: u64,
    /// Explicit `(x, y, w, h)` target applied after the delay, when present.
    pub apply_target: Option<(f64, f64, f64, f64)>,
    /// Grid target `(cols, rows, col, row)` used when explicit geometry is not provided.
    pub apply_grid: Option<(u32, u32, u32, u32)>,
    /// Optional slot identifier used by legacy 2x2 placements.
    pub slot: Option<u8>,
    /// Optional explicit grid specification `(cols, rows, col, row)`.
    pub grid: Option<(u32, u32, u32, u32)>,
    /// Optional explicit inner size `(w, h)` for the helper window.
    pub size: Option<(f64, f64)>,
    /// Optional explicit position `(x, y)` for the helper window.
    pub pos: Option<(f64, f64)>,
    /// Optional overlay label text rendered inside the window.
    pub label_text: Option<String>,
    /// Optional minimum content size `(w, h)`.
    pub min_size: Option<(f64, f64)>,
    /// Optional rounding step `(w, h)` applied to requested sizes.
    pub step_size: Option<(f64, f64)>,
    /// Scenario slug used for diagnostics and sibling lookups.
    pub scenario_slug: Arc<str>,
    /// Helper-specific label used for diagnostics and artifacts.
    pub window_label: Arc<str>,
    /// Launch the helper window minimized when true.
    pub start_minimized: bool,
    /// Launch the helper window zoomed when true.
    pub start_zoomed: bool,
    /// Prevent manual movement of the window when true.
    pub panel_nonmovable: bool,
    /// Prevent manual resizing of the window when true.
    pub panel_nonresizable: bool,
    /// Attach a modal sheet to the helper window when true.
    pub attach_sheet: bool,
    /// Active quirk list applied to the helper runtime.
    pub quirks: Vec<Quirk>,
    /// Placement strategy carried alongside the window for diagnostics.
    pub place: PlaceOptions,
    /// Shutdown flag shared with the controlling harness.
    pub shutdown: Arc<AtomicBool>,
}

impl HelperConfig {
    /// Replace the shutdown flag while preserving other configuration options.
    #[must_use]
    pub fn with_shutdown(mut self, shutdown: Arc<AtomicBool>) -> Self {
        self.shutdown = shutdown;
        self
    }

    /// Attach a new quirk list to the configuration.
    #[must_use]
    pub fn with_quirks(mut self, quirks: Vec<Quirk>) -> Self {
        self.quirks = quirks;
        self
    }
}

impl Default for HelperConfig {
    fn default() -> Self {
        Self {
            time_ms: 15_000,
            delay_setframe_ms: 0,
            delay_apply_ms: 0,
            tween_ms: 0,
            apply_target: None,
            apply_grid: None,
            slot: None,
            grid: None,
            size: None,
            pos: None,
            label_text: None,
            min_size: None,
            step_size: None,
            scenario_slug: Arc::from(""),
            window_label: Arc::from(""),
            start_minimized: false,
            start_zoomed: false,
            panel_nonmovable: false,
            panel_nonresizable: false,
            attach_sheet: false,
            quirks: Vec::new(),
            place: PlaceOptions::default(),
            shutdown: Arc::new(AtomicBool::new(false)),
        }
    }
}

fn run_helper_window(title: String, config: HelperConfig) -> Result<(), String> {
    helper_app::run(title, config)
}

fn apply_quirk_defaults(config: &mut HelperConfig, quirks: &[Quirk]) {
    if quirks.iter().any(|q| matches!(q, Quirk::DelayApplyMove)) && config.delay_apply_ms == 0 {
        config.delay_apply_ms = 160;
    }
    if quirks.iter().any(|q| matches!(q, Quirk::AxRounding)) && config.step_size.is_none() {
        config.step_size = Some((1.0, 1.0));
    }
}

/// Target rect as ((x,y), (w,h), name) used for tween targets.
type TargetRect = ((f64, f64), (f64, f64), &'static str);

/// Internal helper application state used to drive the smoketest helper window.
mod helper_app {
    use std::{
        cmp::min,
        process::id,
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
        thread,
        time::{Duration, Instant},
    };

    use hotki_world_ids::WorldWindowId;
    use mac_winops::{self, AxProps, Rect, screen};
    use objc2::rc::autoreleasepool;
    use tracing::{debug, info};
    use winit::{
        application::ApplicationHandler,
        dpi::{LogicalPosition, LogicalSize, PhysicalPosition, PhysicalSize},
        event::WindowEvent,
        event_loop::{ActiveEventLoop, ControlFlow},
        window::{Window, WindowId},
    };

    use super::{
        HelperConfig, PlaceOptions, Quirk, RaiseStrategy, TargetRect, config, format_quirks,
        parse_decorated_label, select_sibling_for_cycle, should_skip_apply_for_minimized, world,
    };
    use crate::PlaceAttemptOptions;

    /// Parameter bundle for constructing a [`HelperApp`].
    pub(super) struct HelperParams {
        /// Window title shown on the helper surface.
        pub(super) title: String,
        /// Scenario slug used for diagnostics.
        pub(super) scenario_slug: Arc<str>,
        /// Window label tied to artifacts and diagnostics.
        pub(super) window_label: Arc<str>,
        /// Total runtime for the helper window before forced shutdown.
        pub(super) time_ms: u64,
        /// Delay before applying position updates when directly setting frames.
        pub(super) delay_setframe_ms: u64,
        /// Delay before invoking the primary placement operation.
        pub(super) delay_apply_ms: u64,
        /// Duration for tweened placement animations.
        pub(super) tween_ms: u64,
        /// Absolute placement target rectangle, when provided.
        pub(super) apply_target: Option<(f64, f64, f64, f64)>,
        /// Grid placement specification, when provided.
        pub(super) apply_grid: Option<(u32, u32, u32, u32)>,
        /// Optional slot identifier for 2x2 layouts.
        pub(super) slot: Option<u8>,
        /// Optional grid dimensions and coordinates.
        pub(super) grid: Option<(u32, u32, u32, u32)>,
        /// Optional explicit window size.
        pub(super) size: Option<(f64, f64)>,
        /// Optional explicit window position.
        pub(super) pos: Option<(f64, f64)>,
        /// Optional overlay label text.
        pub(super) label_text: Option<String>,
        /// Optional minimum content size.
        pub(super) min_size: Option<(f64, f64)>,
        /// Optional rounding step for requested sizes.
        pub(super) step_size: Option<(f64, f64)>,
        /// Whether the helper launches minimized.
        pub(super) start_minimized: bool,
        /// Whether the helper launches zoomed (macOS zoom behavior).
        pub(super) start_zoomed: bool,
        /// Whether the helper window should be non-movable.
        pub(super) panel_nonmovable: bool,
        /// Whether the helper window should be non-resizable.
        pub(super) panel_nonresizable: bool,
        /// Whether to attach a modal sheet on launch.
        pub(super) attach_sheet: bool,
        /// Quirk list influencing runtime behaviour.
        pub(super) quirks: Vec<Quirk>,
        /// Placement strategy applied to this helper.
        pub(super) place: PlaceOptions,
        /// Shutdown flag shared with external callers.
        pub(super) shutdown: Arc<AtomicBool>,
    }

    /// State machine orchestrating the smoketest helper window lifecycle.
    pub(super) struct HelperApp {
        /// Handle to the helper window, if created.
        window: Option<Window>,
        /// Window title used to locate the NSWindow for tweaks.
        title: String,
        /// Scenario slug for diagnostics.
        scenario_slug: Arc<str>,
        /// Helper label for diagnostics.
        window_label: Arc<str>,
        /// Time at which the helper should terminate.
        deadline: Instant,
        /// Delay before applying a set_frame operation (ms).
        delay_setframe_ms: u64,
        /// Delay before applying the main placement (ms).
        delay_apply_ms: u64,
        /// Tween duration for animated moves (ms).
        tween_ms: u64,
        /// Explicit target rect to apply, if present.
        apply_target: Option<(f64, f64, f64, f64)>,
        /// Grid parameters to compute target rect, if present.
        apply_grid: Option<(u32, u32, u32, u32)>,
        // Async-frame state
        /// Last observed window position.
        last_pos: Option<(f64, f64)>,
        /// Last observed window size.
        last_size: Option<(f64, f64)>,
        /// Desired position requested by the test.
        desired_pos: Option<(f64, f64)>,
        /// Desired size requested by the test.
        desired_size: Option<(f64, f64)>,
        /// Time at which to apply pending placement.
        apply_after: Option<Instant>,
        // Tween state
        /// Whether a tween animation is currently active.
        tween_active: bool,
        /// Tween start time.
        tween_start: Option<Instant>,
        /// Tween end time.
        tween_end: Option<Instant>,
        /// Starting position for tween.
        tween_from_pos: Option<(f64, f64)>,
        /// Starting size for tween.
        tween_from_size: Option<(f64, f64)>,
        /// Target position for tween.
        tween_to_pos: Option<(f64, f64)>,
        /// Target size for tween.
        tween_to_size: Option<(f64, f64)>,
        /// Suppress processing of window events while applying changes.
        suppress_events: bool,
        /// Optional 2x2 slot for placement.
        slot: Option<u8>,
        /// Optional grid spec for placement.
        grid: Option<(u32, u32, u32, u32)>,
        /// Optional explicit initial size.
        size: Option<(f64, f64)>,
        /// Optional explicit initial position.
        pos: Option<(f64, f64)>,
        /// Optional label text to display.
        label_text: Option<String>,
        /// Optional minimum content size.
        min_size: Option<(f64, f64)>,
        /// Fatal error encountered during setup.
        error: Option<String>,
        /// Start minimized if requested.
        start_minimized: bool,
        /// Start zoomed (macOS “zoomed”) if requested.
        start_zoomed: bool,
        /// Make the panel non-movable if requested.
        panel_nonmovable: bool,
        /// Make the panel non-resizable if requested.
        panel_nonresizable: bool,
        /// Attach a modal sheet to the helper window if requested.
        attach_sheet: bool,
        // Optional: round requested sizes to nearest multiples
        /// Width rounding step for requested sizes.
        step_w: f64,
        /// Height rounding step for requested sizes.
        step_h: f64,
        /// Shutdown flag toggled by the harness to request exit.
        shutdown: Arc<AtomicBool>,
        /// Active quirk list applied to this helper window.
        quirks: Vec<Quirk>,
        /// Placement options for raise/minimize behaviour.
        place: PlaceOptions,
    }

    impl HelperApp {
        fn has_quirk(&self, quirk: Quirk) -> bool {
            self.quirks.contains(&quirk)
        }

        fn diag_tag(&self) -> String {
            format!(
                "{}/{} quirks=[{}]",
                self.scenario_slug.as_ref(),
                self.window_label.as_ref(),
                format_quirks(&self.quirks)
            )
        }

        /// Build a helper app with the provided configuration snapshot.
        pub(super) fn new(params: HelperParams) -> Self {
            let HelperParams {
                title,
                scenario_slug,
                window_label,
                time_ms,
                delay_setframe_ms,
                delay_apply_ms,
                tween_ms,
                apply_target,
                apply_grid,
                slot,
                grid,
                size,
                pos,
                label_text,
                min_size,
                step_size,
                start_minimized,
                start_zoomed,
                panel_nonmovable,
                panel_nonresizable,
                attach_sheet,
                quirks,
                place,
                shutdown,
            } = params;
            Self {
                window: None,
                title,
                scenario_slug,
                window_label,
                deadline: Instant::now() + config::ms(time_ms.max(1000)),
                delay_setframe_ms,
                delay_apply_ms,
                tween_ms,
                apply_target,
                apply_grid,
                last_pos: None,
                last_size: None,
                desired_pos: None,
                desired_size: None,
                apply_after: None,
                tween_active: false,
                tween_start: None,
                tween_end: None,
                tween_from_pos: None,
                tween_from_size: None,
                tween_to_pos: None,
                tween_to_size: None,
                suppress_events: false,
                slot,
                grid,
                size,
                pos,
                label_text,
                min_size,
                error: None,
                start_minimized,
                start_zoomed,
                panel_nonmovable,
                panel_nonresizable,
                attach_sheet,
                step_w: step_size.map(|s| s.0).unwrap_or(0.0),
                step_h: step_size.map(|s| s.1).unwrap_or(0.0),
                shutdown,
                quirks,
                place,
            }
        }

        /// Return any captured fatal error and clear it from state.
        pub(super) fn take_error(&mut self) -> Option<String> {
            self.error.take()
        }

        /// Create the helper window with initial attributes.
        fn try_create_window(&self, elwt: &ActiveEventLoop) -> Result<Window, String> {
            use winit::dpi::LogicalSize;
            let attrs = Window::default_attributes()
                .with_title(self.title.clone())
                .with_visible(true)
                .with_decorations(false)
                .with_inner_size(LogicalSize::new(
                    self.size
                        .map(|s| s.0)
                        .unwrap_or(config::HELPER_WINDOW.width_px),
                    self.size
                        .map(|s| s.1)
                        .unwrap_or(config::HELPER_WINDOW.height_px),
                ));
            elwt.create_window(attrs).map_err(|e| e.to_string())
        }

        /// Bring the application to the foreground on resume.
        fn activate_app(&self) {
            if let Some(mtm) = objc2_foundation::MainThreadMarker::new() {
                let app = objc2_app_kit::NSApplication::sharedApplication(mtm);
                unsafe { app.activate() };
            }
        }

        /// Enforce the configured minimum content size, if any.
        fn apply_min_size_if_requested(&self) {
            if let Some((min_w, min_h)) = self.min_size
                && let Some(mtm) = objc2_foundation::MainThreadMarker::new()
            {
                use objc2_foundation::NSSize;
                let app = objc2_app_kit::NSApplication::sharedApplication(mtm);
                for w in app.windows().iter() {
                    let t = w.title();
                    let is_match = autoreleasepool(|pool| unsafe { t.to_str(pool) == self.title });
                    if is_match {
                        unsafe {
                            w.setMinSize(NSSize::new(min_w, min_h));
                            w.setContentMinSize(NSSize::new(min_w, min_h));
                        }
                        break;
                    }
                }
            }
        }

        /// Make the panel non-movable if configured.
        fn apply_nonmovable_if_requested(&self) {
            if self.panel_nonmovable
                && let Some(mtm) = objc2_foundation::MainThreadMarker::new()
            {
                let app = objc2_app_kit::NSApplication::sharedApplication(mtm);
                let windows = app.windows();
                for w in windows.iter() {
                    let t = w.title();
                    let is_match = autoreleasepool(|pool| unsafe { t.to_str(pool) == self.title });
                    if is_match {
                        w.setMovable(false);
                        break;
                    }
                }
            }
        }

        /// Make the panel non-resizable if configured.
        fn apply_nonresizable_if_requested(&self) {
            if self.panel_nonresizable
                && let Some(mtm) = objc2_foundation::MainThreadMarker::new()
            {
                use objc2_app_kit::NSWindowStyleMask;
                let app = objc2_app_kit::NSApplication::sharedApplication(mtm);
                let windows = app.windows();
                for w in windows.iter() {
                    let t = w.title();
                    let is_match = autoreleasepool(|pool| unsafe { t.to_str(pool) == self.title });
                    if is_match {
                        let mut mask = w.styleMask();
                        mask.remove(NSWindowStyleMask::Resizable);
                        w.setStyleMask(mask);
                        break;
                    }
                }
            }
        }

        /// Poll the world snapshot to resolve the helper window's identifier.
        fn resolve_world_window(&self) -> Option<WorldWindowId> {
            let pid = id() as i32;
            match world::list_windows() {
                Ok(windows) => windows
                    .into_iter()
                    .find(|w| w.pid == pid && w.title == self.title)
                    .map(|w| WorldWindowId::new(w.pid, w.id)),
                Err(err) => {
                    tracing::debug!("winhelper: world snapshot failed: {}", err);
                    None
                }
            }
        }

        /// Perform the initial placement of the window.
        fn initial_placement(&self, win: &Window) {
            use winit::dpi::LogicalPosition;
            let pid = id() as i32;
            match self.place.raise {
                RaiseStrategy::None | RaiseStrategy::KeepFrontWindow => {
                    debug!(
                        "winhelper: skip ensure_frontmost (raise={:?})",
                        self.place.raise
                    );
                }
                RaiseStrategy::AppActivate => {
                    if let Err(err) = world::ensure_frontmost(
                        pid,
                        &self.title,
                        3,
                        config::INPUT_DELAYS.retry_delay_ms,
                    ) {
                        debug!(
                            "winhelper: ensure_frontmost failed pid={} title='{}': {}",
                            pid, self.title, err
                        );
                    }
                }
            }
            if let Some((cols, rows, col, row)) = self.grid {
                self.try_world_place(cols, rows, col, row, None);
            } else if let Some(slot) = self.slot {
                let (col, row) = match slot {
                    1 => (0, 0),
                    2 => (1, 0),
                    3 => (0, 1),
                    _ => (1, 1),
                };
                self.try_world_place(2, 2, col, row, None);
            } else if let Some((x, y)) = self.pos {
                win.set_outer_position(LogicalPosition::new(x, y));
            } else if let Some(mtm) = objc2_foundation::MainThreadMarker::new() {
                // Fallback: bottom-right corner at a fixed small size on main screen.
                use objc2_app_kit::NSScreen;
                let margin: f64 = config::HELPER_WINDOW.margin_px;
                if let Some(scr) = NSScreen::mainScreen(mtm) {
                    let vf = scr.visibleFrame();
                    let w = config::HELPER_WINDOW.width_px;
                    let x = (vf.origin.x + vf.size.width - w - margin).max(0.0);
                    let y = (vf.origin.y + margin).max(0.0);
                    win.set_outer_position(LogicalPosition::new(x, y));
                }
            }
            // Apply style tweaks after initial placement to avoid interfering with it.
            self.apply_nonresizable_if_requested();
            self.apply_nonmovable_if_requested();
        }

        /// Retry world placement until the helper window is visible to the world snapshot.
        fn try_world_place(
            &self,
            cols: u32,
            rows: u32,
            col: u32,
            row: u32,
            options: Option<&PlaceAttemptOptions>,
        ) {
            for attempt in 0..120 {
                if let Some(target) = self.resolve_world_window() {
                    let pid = target.pid();
                    match world::place_window(target, cols, rows, col, row, options.cloned()) {
                        Ok(_) => {
                            if self.verify_grid_cell(pid, cols, rows, col, row) {
                                return;
                            }
                        }
                        Err(err) => {
                            debug!(
                                "winhelper: world placement attempt {} failed: {}",
                                attempt, err
                            );
                        }
                    }
                }
                thread::sleep(Duration::from_millis(20));
            }
            debug!("winhelper: world placement giving up after retries");
        }

        /// Confirm the helper window occupies the requested grid cell using
        /// anchored semantics (position matches exactly; size may exceed the
        /// cell because of minimums or non-resizable windows).
        fn verify_grid_cell(&self, pid: i32, cols: u32, rows: u32, col: u32, row: u32) -> bool {
            if let Some(((x, y), (w, h))) = mac_winops::ax_window_frame(pid, &self.title)
                && let Some(vf) = screen::visible_frame_containing_point(x, y)
            {
                let expected = mac_winops::cell_rect(vf, cols, rows, col, row);
                let eps = config::PLACE.eps;
                let pos_ok = (x - expected.x).abs() <= eps && (y - expected.y).abs() <= eps;
                let size_ok = (w + eps) >= expected.w && (h + eps) >= expected.h;
                return pos_ok && size_ok;
            }
            false
        }

        /// Capture the starting geometry used by delayed/tweened placement logic.
        fn capture_initial_geometry(&mut self, win: &Window) {
            let scale = win.scale_factor();
            if let Ok(p) = win.outer_position() {
                let lp = p.to_logical::<f64>(scale);
                self.last_pos = Some((lp.x, lp.y));
            }
            let isz = win.inner_size();
            let lsz = isz.to_logical::<f64>(scale);
            self.last_size = Some((lsz.width, lsz.height));
        }

        /// Arm delayed application if explicitly configured.
        fn arm_delayed_apply_if_configured(&mut self) {
            if self.delay_apply_ms > 0 {
                self.apply_after = Some(Instant::now() + config::ms(self.delay_apply_ms));
                debug!(
                    "winhelper: armed delayed-apply +{}ms target={:?} grid={:?}",
                    self.delay_apply_ms, self.apply_target, self.apply_grid
                );
            }
        }

        /// Apply initial zoom/minimize state if requested.
        fn apply_initial_state_options(&self) {
            if let Some(mtm) = objc2_foundation::MainThreadMarker::new() {
                let app = objc2_app_kit::NSApplication::sharedApplication(mtm);
                let windows = app.windows();
                for w in windows.iter() {
                    let t = w.title();
                    let is_match = autoreleasepool(|pool| unsafe { t.to_str(pool) == self.title });
                    if is_match {
                        unsafe {
                            if self.start_zoomed && !w.isZoomed() {
                                w.performZoom(None);
                            }
                            if self.start_minimized && !w.isMiniaturized() {
                                w.miniaturize(None);
                            }
                        }
                        break;
                    }
                }
            }
        }

        fn window_is_minimized(&self) -> bool {
            if let Some(mtm) = objc2_foundation::MainThreadMarker::new() {
                let app = objc2_app_kit::NSApplication::sharedApplication(mtm);
                let windows = app.windows();
                for w in windows.iter() {
                    let title = w.title();
                    let is_match =
                        autoreleasepool(|pool| unsafe { title.to_str(pool) == self.title });
                    if is_match {
                        return w.isMiniaturized();
                    }
                }
            }
            false
        }

        fn apply_ax_rounding_override(&self) {
            if !self.has_quirk(Quirk::AxRounding) {
                return;
            }
            if let Some(target) = self.resolve_world_window()
                && let Some(win) = self.window.as_ref()
            {
                let scale = win.scale_factor();
                if let Ok(pos) = win.outer_position() {
                    let lp = pos.to_logical::<f64>(scale);
                    let size = win.inner_size().to_logical::<f64>(scale);
                    let rect = Rect {
                        x: lp.x.floor(),
                        y: lp.y.floor(),
                        w: size.width.floor(),
                        h: size.height.floor(),
                    };
                    let props = AxProps {
                        role: None,
                        subrole: None,
                        can_set_pos: Some(true),
                        can_set_size: Some(true),
                        frame: Some(rect),
                        minimized: Some(false),
                        fullscreen: Some(false),
                        visible: Some(true),
                        zoomed: Some(false),
                    };
                    crate::test_api::set_ax_props(target.pid(), target.window_id(), props);
                }
            }
        }

        /// Add a large centered label to the content view.
        fn add_centered_label(&self) {
            if let Some(mtm) = objc2_foundation::MainThreadMarker::new() {
                let app = objc2_app_kit::NSApplication::sharedApplication(mtm);
                let windows = app.windows();
                for w in windows.iter() {
                    let title = w.title();
                    let is_match =
                        autoreleasepool(|pool| unsafe { title.to_str(pool) == self.title });
                    if is_match {
                        if let Some(view) = w.contentView() {
                            use objc2::rc::Retained;
                            use objc2_app_kit::{NSColor, NSFont, NSTextAlignment, NSTextField};
                            use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};
                            let label_str = self.compute_label_text();
                            let ns = NSString::from_str(&label_str);
                            let label: Retained<NSTextField> =
                                unsafe { NSTextField::labelWithString(&ns, mtm) };
                            let vframe = view.frame();
                            let vw = vframe.size.width;
                            let vh = vframe.size.height;
                            let base = vw.min(vh) * 0.35;
                            let font = unsafe { NSFont::boldSystemFontOfSize(base) };
                            unsafe { label.setFont(Some(&font)) };
                            unsafe { label.setAlignment(NSTextAlignment::Center) };
                            let color = unsafe { NSColor::whiteColor() };
                            unsafe { label.setTextColor(Some(&color)) };
                            let margin_x = 8.0;
                            let margin_y = 8.0;
                            let lw = (vw - 2.0 * margin_x).max(10.0);
                            let lh = (vh - 2.0 * margin_y).max(20.0);
                            let lx = vframe.origin.x + margin_x;
                            let ly = vframe.origin.y + margin_y;
                            let frame = NSRect::new(NSPoint::new(lx, ly), NSSize::new(lw, lh));
                            unsafe { label.setFrame(frame) };
                            unsafe { view.addSubview(&label) };
                        }
                        break;
                    }
                }
            }
        }

        /// Handle a `WindowEvent::Moved`.
        fn on_moved(&mut self, new_pos: PhysicalPosition<i32>) {
            use winit::dpi::LogicalPosition;
            debug!("winhelper: moved event: x={} y={}", new_pos.x, new_pos.y);
            let intercept =
                (self.delay_setframe_ms > 0 || self.delay_apply_ms > 0 || self.tween_ms > 0)
                    && !self.suppress_events;
            if intercept {
                if let Some(win) = self.window.as_ref() {
                    let scale = win.scale_factor();
                    let lp = new_pos.to_logical::<f64>(scale);
                    if self.last_pos.is_none()
                        && let Ok(p0) = win.outer_position()
                    {
                        let p0l = p0.to_logical::<f64>(scale);
                        self.last_pos = Some((p0l.x, p0l.y));
                    }
                    self.desired_pos = Some((lp.x, lp.y));
                    debug!(
                        "winhelper: intercept move -> desired=({:.1},{:.1}) last={:?}",
                        lp.x, lp.y, self.last_pos
                    );
                    if let Some((x, y)) = self.last_pos {
                        self.suppress_events = true;
                        win.set_outer_position(LogicalPosition::new(x, y));
                        self.suppress_events = false;
                    }
                    if self.tween_ms > 0 {
                        if self.delay_apply_ms > 0
                            && (self.apply_target.is_some() || self.apply_grid.is_some())
                        {
                            self.apply_after =
                                Some(Instant::now() + config::ms(self.delay_apply_ms));
                        } else {
                            let now = Instant::now();
                            self.ensure_tween_started_pos(now);
                            self.tween_to_pos = self.desired_pos;
                            self.apply_after = Some(now);
                        }
                    } else if self.delay_apply_ms > 0 {
                        self.apply_after = Some(Instant::now() + config::ms(self.delay_apply_ms));
                        debug!(
                            "winhelper: scheduled apply_after at +{}ms (delay_apply)",
                            self.delay_apply_ms
                        );
                    } else {
                        self.apply_after =
                            Some(Instant::now() + config::ms(self.delay_setframe_ms));
                        debug!(
                            "winhelper: scheduled apply_after at +{}ms (delay_setframe)",
                            self.delay_setframe_ms
                        );
                    }
                }
            } else if !self.suppress_events
                && let Some(win) = self.window.as_ref()
            {
                let scale = win.scale_factor();
                let lp = new_pos.to_logical::<f64>(scale);
                self.last_pos = Some((lp.x, lp.y));
                debug!("winhelper: track move -> last=({:.1},{:.1})", lp.x, lp.y);
            }
        }

        fn cycle_focus_to_sibling_if_needed(&self) {
            if self.place.raise != RaiseStrategy::KeepFrontWindow {
                return;
            }
            if !self.has_quirk(Quirk::RaiseCyclesToSibling) {
                return;
            }
            let pid = id() as i32;
            match world::list_windows() {
                Ok(windows) => {
                    let slug_fragment = format!("[{}::", self.scenario_slug.as_ref());
                    if let Some(sibling) =
                        select_sibling_for_cycle(&windows, pid, &self.title, &slug_fragment)
                    {
                        let sibling_label = parse_decorated_label(&sibling.title)
                            .unwrap_or("?")
                            .to_string();
                        self.raise_sibling(sibling.pid, sibling.id, sibling_label);
                    }
                }
                Err(err) => debug!(
                    tag = %self.diag_tag(),
                    error = %err,
                    "failed to list windows during focus cycle"
                ),
            }
        }

        fn raise_sibling(&self, pid: i32, id: u32, sibling_label: String) {
            let sibling_tag = format!("{}/{}", self.scenario_slug.as_ref(), sibling_label);
            match mac_winops::raise_window(pid, id) {
                Ok(()) => {
                    debug!(
                        tag = %self.diag_tag(),
                        sibling = %sibling_tag,
                        "cycled focus to sibling"
                    );
                }
                Err(err) => debug!(
                    tag = %self.diag_tag(),
                    sibling = %sibling_tag,
                    error = %err,
                    "failed to raise sibling during focus cycle"
                ),
            }
        }

        /// Handle a `WindowEvent::Resized`.
        fn on_resized(&mut self, new_size: PhysicalSize<u32>) {
            use winit::dpi::LogicalSize;
            debug!(
                "winhelper: resized event: w={} h={}",
                new_size.width, new_size.height
            );
            let intercept =
                (self.delay_setframe_ms > 0 || self.delay_apply_ms > 0 || self.tween_ms > 0)
                    && !self.suppress_events;
            if intercept {
                if let Some(win) = self.window.as_ref() {
                    let scale = win.scale_factor();
                    let lsz = new_size.to_logical::<f64>(scale);
                    if self.last_size.is_none() {
                        let s0 = win.inner_size().to_logical::<f64>(scale);
                        self.last_size = Some((s0.width, s0.height));
                    }
                    self.desired_size = Some((lsz.width, lsz.height));
                    debug!(
                        "winhelper: intercept resize -> desired=({:.1},{:.1}) last={:?}",
                        lsz.width, lsz.height, self.last_size
                    );
                    if let Some((w, h)) = self.last_size {
                        self.suppress_events = true;
                        let _maybe_size = win.request_inner_size(LogicalSize::new(w, h));
                        self.suppress_events = false;
                    }
                    if self.tween_ms > 0 {
                        if self.delay_apply_ms > 0
                            && (self.apply_target.is_some() || self.apply_grid.is_some())
                        {
                            self.apply_after =
                                Some(Instant::now() + config::ms(self.delay_apply_ms));
                        } else {
                            let now = Instant::now();
                            self.ensure_tween_started_size(now);
                            self.tween_to_size = self.desired_size;
                            self.apply_after = Some(now);
                        }
                    } else if self.delay_apply_ms > 0 {
                        self.apply_after = Some(Instant::now() + config::ms(self.delay_apply_ms));
                        debug!(
                            "winhelper: scheduled apply_after at +{}ms (delay_apply)",
                            self.delay_apply_ms
                        );
                    } else {
                        self.apply_after =
                            Some(Instant::now() + config::ms(self.delay_setframe_ms));
                        debug!(
                            "winhelper: scheduled apply_after at +{}ms (delay_setframe)",
                            self.delay_setframe_ms
                        );
                    }
                }
            } else if !self.suppress_events
                && let Some(win) = self.window.as_ref()
            {
                let scale = win.scale_factor();
                let lsz = new_size.to_logical::<f64>(scale);
                self.last_size = Some((lsz.width, lsz.height));
                debug!(
                    "winhelper: track resize -> last=({:.1},{:.1})",
                    lsz.width, lsz.height
                );
            }
        }

        /// Handle a `WindowEvent::Focused`.
        fn on_focused(&self, focused: bool) {
            info!(title = %self.title, focused, "winhelper: focus event");
            if let Some(mtm) = objc2_foundation::MainThreadMarker::new() {
                let app = objc2_app_kit::NSApplication::sharedApplication(mtm);
                let windows = app.windows();
                for w in windows.iter() {
                    let title = w.title();
                    let is_match =
                        autoreleasepool(|pool| unsafe { title.to_str(pool) == self.title });
                    if is_match {
                        let color = unsafe {
                            if focused {
                                objc2_app_kit::NSColor::systemBlueColor()
                            } else {
                                objc2_app_kit::NSColor::controlBackgroundColor()
                            }
                        };
                        w.setBackgroundColor(Some(&color));
                        break;
                    }
                }
            }
            if focused {
                self.cycle_focus_to_sibling_if_needed();
            }
        }
        /// Return the visible frame of the screen containing the given window.
        fn active_visible_frame_for_window(&self, win: &Window) -> (f64, f64, f64, f64) {
            if let Some(mtm) = objc2_foundation::MainThreadMarker::new() {
                use objc2_app_kit::NSScreen;
                let scale = win.scale_factor();
                let p = win
                    .outer_position()
                    .ok()
                    .map(|p| p.to_logical::<f64>(scale))
                    .unwrap_or(LogicalPosition::new(0.0, 0.0));
                let mut chosen: Option<(f64, f64, f64, f64)> = None;
                for s in NSScreen::screens(mtm).iter() {
                    let fr = s.visibleFrame();
                    let sx = fr.origin.x;
                    let sy = fr.origin.y;
                    let sw = fr.size.width;
                    let sh = fr.size.height;
                    if p.x >= sx && p.x <= sx + sw && p.y >= sy && p.y <= sy + sh {
                        chosen = Some((sx, sy, sw, sh));
                        break;
                    }
                }
                if let Some(v) = chosen {
                    return v;
                }
                if let Some(scr) = NSScreen::mainScreen(mtm) {
                    let r = scr.visibleFrame();
                    return (r.origin.x, r.origin.y, r.size.width, r.size.height);
                }
            }
            (0.0, 0.0, 1440.0, 900.0)
        }

        /// Compute the rectangle for a tile within a grid on the active screen.
        fn grid_rect(
            &self,
            win: &Window,
            cols: u32,
            rows: u32,
            col: u32,
            row: u32,
        ) -> (f64, f64, f64, f64) {
            let (vf_x, vf_y, vf_w, vf_h) = self.active_visible_frame_for_window(win);
            let c = cols.max(1) as f64;
            let r = rows.max(1) as f64;
            let tile_w = (vf_w / c).floor().max(1.0);
            let tile_h = (vf_h / r).floor().max(1.0);
            let rem_w = vf_w - tile_w * (cols as f64);
            let rem_h = vf_h - tile_h * (rows as f64);
            let tx = vf_x + tile_w * (col as f64);
            let tw = if col == cols.saturating_sub(1) {
                tile_w + rem_w
            } else {
                tile_w
            };
            let ty = vf_y + tile_h * (row as f64);
            let th = if row == rows.saturating_sub(1) {
                tile_h + rem_h
            } else {
                tile_h
            };
            (tx, ty, tw, th)
        }

        /// Compute the label text to display in the helper window.
        /// Helper selectors are inlined at call sites to avoid borrow conflicts.
        fn compute_label_text(&self) -> String {
            if let Some(ref t) = self.label_text {
                return t.clone();
            }
            if let Some((cols, rows, col, row)) = self.grid {
                if cols == 2 && rows == 2 {
                    return match (col, row) {
                        (0, 0) => "TL".into(),
                        (1, 0) => "TR".into(),
                        (0, 1) => "BL".into(),
                        _ => "BR".into(),
                    };
                }
                return self.title.clone();
            }
            if let Some(slot) = self.slot {
                return match slot {
                    1 => "TL".into(),
                    2 => "TR".into(),
                    3 => "BL".into(),
                    _ => "BR".into(),
                };
            }
            self.title.clone()
        }

        /// Ensure tween state is initialized for position changes.
        fn ensure_tween_started_pos(&mut self, now: Instant) {
            if !self.tween_active {
                self.tween_active = true;
                self.tween_start = Some(now);
                self.tween_end = Some(now + config::ms(self.tween_ms));
                self.tween_from_pos = self.last_pos;
            }
        }

        /// Ensure tween state is initialized for size changes.
        fn ensure_tween_started_size(&mut self, now: Instant) {
            if !self.tween_active {
                self.tween_active = true;
                self.tween_start = Some(now);
                self.tween_end = Some(now + config::ms(self.tween_ms));
                self.tween_from_size = self.last_size;
            }
        }

        /// Optionally round a size to the configured step.
        fn rounded_size(&self, w: f64, h: f64) -> (f64, f64) {
            if self.step_w > 0.0 && self.step_h > 0.0 {
                (
                    (w / self.step_w).round() * self.step_w,
                    (h / self.step_h).round() * self.step_h,
                )
            } else {
                (w, h)
            }
        }

        /// Select the target rectangle to apply (explicit target, grid, or desired geometry).
        fn select_apply_target(&self, win: &Window) -> Option<TargetRect> {
            if let Some((x, y, w, h)) = self.apply_target {
                return Some(((x, y), (w, h), "target"));
            }
            if let Some((c, r, ic, ir)) = self.apply_grid {
                let (tx, ty, tw, th) = self.grid_rect(win, c, r, ic, ir);
                return Some(((tx, ty), (tw, th), "grid"));
            }
            let pos = self.desired_pos.or(self.last_pos);
            let size = self.desired_size.or(self.last_size);
            match (pos, size) {
                (Some(p), Some(s)) => Some((p, s, "desired")),
                _ => None,
            }
        }

        /// Initialize tween destination from a target rectangle.
        fn set_tween_target_from(&mut self, target: TargetRect) {
            let ((x, y), (w, h), kind) = target;
            self.tween_to_pos = Some((x, y));
            self.tween_to_size = Some((w, h));
            debug!(
                "winhelper: tween-start ({}) -> ({:.1},{:.1},{:.1},{:.1})",
                kind, x, y, w, h
            );
        }

        /// Compute tween progress in the range [0.0, 1.0].
        fn tween_progress(&self, now: Instant) -> f64 {
            let start = match self.tween_start {
                Some(s) => s,
                None => return 1.0,
            };
            let end = match self.tween_end {
                Some(e) => e,
                None => return 1.0,
            };
            let total = end.saturating_duration_since(start);
            if total.as_millis() == 0 {
                1.0
            } else {
                let elapsed = now.saturating_duration_since(start).as_secs_f64();
                (elapsed / total.as_secs_f64()).clamp(0.0, 1.0)
            }
        }

        /// Interpolate position and size based on tween progress `t`.
        fn tween_interpolate(&self, t: f64) -> (f64, f64, f64, f64) {
            let (mut nx, mut ny) = self.last_pos.unwrap_or((0.0, 0.0));
            let (mut nw, mut nh) = self.last_size.unwrap_or((
                config::HELPER_WINDOW.width_px,
                config::HELPER_WINDOW.height_px,
            ));
            if let (Some((fx, fy)), Some((tx, ty))) = (self.tween_from_pos, self.tween_to_pos) {
                nx = fx + (tx - fx) * t;
                ny = fy + (ty - fy) * t;
            }
            if let (Some((fw, fh)), Some((tw, th))) = (self.tween_from_size, self.tween_to_size) {
                nw = fw + (tw - fw) * t;
                nh = fh + (th - fh) * t;
            }
            (nx, ny, nw, nh)
        }

        /// Revert minor drift that may occur while testing async placement.
        fn revert_drift_if_needed(&mut self) {
            if let Some(win) = self.window.as_ref()
                && let (Some((lx, ly)), Some((lw, lh))) = (self.last_pos, self.last_size)
            {
                let scale = win.scale_factor();
                let p = win
                    .outer_position()
                    .ok()
                    .map(|p| p.to_logical::<f64>(scale));
                let s = win.inner_size().to_logical::<f64>(scale);
                if let Some(p) = p {
                    let dx = (p.x - lx).abs();
                    let dy = (p.y - ly).abs();
                    let dw = (s.width - lw).abs();
                    let dh = (s.height - lh).abs();
                    if dx > 0.5 || dy > 0.5 || dw > 0.5 || dh > 0.5 {
                        debug!(
                            "winhelper: revert drift dx={:.1} dy={:.1} dw={:.1} dh={:.1}",
                            dx, dy, dw, dh
                        );
                        self.suppress_events = true;
                        let _maybe_size = win.request_inner_size(LogicalSize::new(lw, lh));
                        win.set_outer_position(LogicalPosition::new(lx, ly));
                        self.suppress_events = false;
                    }
                }
            }
        }

        /// Apply a single tween step, updating window position/size.
        fn apply_tween_step(&mut self) {
            if should_skip_apply_for_minimized(&self.quirks, self.window_is_minimized()) {
                debug!("winhelper: skip tween apply while minimized");
                self.apply_after = None;
                return;
            }
            let now = Instant::now();
            if let Some(win) = self.window.as_ref() {
                let target = self.select_apply_target(win);
                if !self.tween_active {
                    self.tween_active = true;
                    self.tween_start = Some(now);
                    self.tween_end = Some(now + config::ms(self.tween_ms));
                    self.tween_from_pos = self.last_pos;
                    self.tween_from_size = self.last_size;
                    if let Some(target) = target {
                        self.set_tween_target_from(target);
                    }
                }
            }
            if let Some(win2) = self.window.as_ref() {
                let t = self.tween_progress(now);
                let (nx, ny, nw, nh) = self.tween_interpolate(t);
                let (rw, rh) = self.rounded_size(nw, nh);
                let _maybe_size = win2.request_inner_size(LogicalSize::new(rw, rh));
                win2.set_outer_position(LogicalPosition::new(nx, ny));
            }
            if self.tween_start.is_some() && self.tween_end.is_some() {
                let t_done = self.tween_progress(Instant::now());
                if (t_done - 1.0).abs() < f64::EPSILON {
                    if let Some((w, h)) = self.tween_to_size {
                        let (_rw, _rh) = self.rounded_size(w, h);
                        self.last_size = Some((w, h));
                    }
                    self.last_pos = self.tween_to_pos.or(self.last_pos);
                    self.tween_active = false;
                    self.tween_start = None;
                    self.tween_end = None;
                    self.tween_from_pos = None;
                    self.tween_from_size = None;
                    self.tween_to_pos = None;
                    self.tween_to_size = None;
                    self.apply_after = None;
                } else {
                    self.apply_after = Some(now + config::ms(16));
                }
            }
            self.apply_ax_rounding_override();
        }

        /// Apply target geometry immediately without tweening.
        fn apply_immediate(&mut self) {
            if should_skip_apply_for_minimized(&self.quirks, self.window_is_minimized()) {
                debug!("winhelper: skip immediate apply while minimized");
                self.apply_after = None;
                return;
            }
            if let Some(win) = self.window.as_ref() {
                if let Some((x, y, w, h)) = self.apply_target {
                    let (rw, rh) = self.rounded_size(w, h);
                    // Ignore the returned size; it is advisory for winit.
                    let _maybe_size = win.request_inner_size(LogicalSize::new(rw, rh));
                    win.set_outer_position(LogicalPosition::new(x, y));
                    self.last_pos = Some((x, y));
                    self.last_size = Some((rw, rh));
                    debug!(
                        "winhelper: explicit apply (explicit) -> ({:.1},{:.1},{:.1},{:.1})",
                        x, y, rw, rh
                    );
                } else if let Some((cols, rows, col, row)) = self.apply_grid {
                    let (tx, ty, tw, th) = self.grid_rect(win, cols, rows, col, row);
                    let (rw, rh) = self.rounded_size(tw, th);
                    let _maybe_size = win.request_inner_size(LogicalSize::new(rw, rh));
                    win.set_outer_position(LogicalPosition::new(tx, ty));
                    self.last_pos = Some((tx, ty));
                    self.last_size = Some((rw, rh));
                    debug!(
                        "winhelper: explicit apply (grid) -> ({:.1},{:.1},{:.1},{:.1})",
                        tx, ty, rw, rh
                    );
                } else {
                    let desired_size_val = self.desired_size;
                    let desired_pos_val = self.desired_pos;
                    if let Some(win2) = self.window.as_ref() {
                        if let Some((w, h)) = desired_size_val {
                            let (rw, rh) = self.rounded_size(w, h);
                            let _maybe_size = win2.request_inner_size(LogicalSize::new(rw, rh));
                            self.last_size = Some((rw, rh));
                            self.desired_size = None;
                        }
                        if let Some((x, y)) = desired_pos_val {
                            win2.set_outer_position(LogicalPosition::new(x, y));
                            self.last_pos = Some((x, y));
                            self.desired_pos = None;
                        }
                        debug!("winhelper: applied desired pos/size");
                    }
                }
            }
            // Ensure we clear any pending apply-after state after applying.
            self.apply_after = None;
            self.apply_ax_rounding_override();
        }

        /// Apply placement when the apply deadline has been reached.
        fn process_apply_ready(&mut self) {
            if self.window.is_none() {
                self.apply_after = None;
                return;
            }
            self.suppress_events = true;
            if self.tween_ms > 0 {
                self.apply_tween_step();
            } else {
                self.apply_immediate();
            }
            self.suppress_events = false;
        }
    }

    impl ApplicationHandler for HelperApp {
        fn resumed(&mut self, elwt: &ActiveEventLoop) {
            if self.window.is_some() {
                return;
            }
            let win = match self.try_create_window(elwt) {
                Ok(w) => w,
                Err(e) => {
                    self.error = Some(format!("winhelper: failed to create window: {}", e));
                    elwt.exit();
                    return;
                }
            };
            self.activate_app();
            self.apply_min_size_if_requested();
            self.apply_nonmovable_if_requested();
            self.initial_placement(&win);
            // Allow registration to settle before adding label.
            thread::sleep(config::ms(
                config::INPUT_DELAYS.window_registration_delay_ms,
            ));
            self.capture_initial_geometry(&win);
            self.arm_delayed_apply_if_configured();
            let _ = self.attach_sheet; // placeholder hook
            self.apply_initial_state_options();
            self.add_centered_label();
            self.window = Some(win);
        }
        fn window_event(&mut self, elwt: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
            match event {
                WindowEvent::CloseRequested => elwt.exit(),
                WindowEvent::Moved(pos) => self.on_moved(pos),
                WindowEvent::Resized(sz) => self.on_resized(sz),
                WindowEvent::Focused(f) => self.on_focused(f),
                _ => {}
            }
        }
        fn about_to_wait(&mut self, elwt: &ActiveEventLoop) {
            if self.shutdown.load(Ordering::SeqCst) {
                elwt.exit();
                return;
            }
            let now = Instant::now();
            if now >= self.deadline {
                elwt.exit();
                return;
            }
            match self.apply_after {
                Some(when) if now < when => self.revert_drift_if_needed(),
                _ => self.process_apply_ready(),
            }
            // Wake up at the next interesting time (apply_after or final deadline)
            let next = match self.apply_after {
                Some(t) => min(t, self.deadline),
                None => self.deadline,
            };
            elwt.set_control_flow(ControlFlow::WaitUntil(next));
        }
    }

    impl HelperParams {
        pub(super) fn from_config(title: String, config: HelperConfig) -> Self {
            let HelperConfig {
                time_ms,
                delay_setframe_ms,
                delay_apply_ms,
                tween_ms,
                apply_target,
                apply_grid,
                slot,
                grid,
                size,
                pos,
                label_text,
                min_size,
                step_size,
                scenario_slug,
                window_label,
                start_minimized,
                start_zoomed,
                panel_nonmovable,
                panel_nonresizable,
                attach_sheet,
                quirks,
                place,
                shutdown,
            } = config;

            Self {
                title,
                scenario_slug,
                window_label,
                time_ms,
                delay_setframe_ms,
                delay_apply_ms,
                tween_ms,
                apply_target,
                apply_grid,
                slot,
                grid,
                size,
                pos,
                label_text,
                min_size,
                step_size,
                start_minimized,
                start_zoomed,
                panel_nonmovable,
                panel_nonresizable,
                attach_sheet,
                quirks,
                place,
                shutdown,
            }
        }
    }

    pub(super) fn run(title: String, config: HelperConfig) -> Result<(), String> {
        use winit::event_loop::EventLoop;

        let params = HelperParams::from_config(title, config);
        let event_loop = EventLoop::new().map_err(|e| e.to_string())?;
        let mut app = HelperApp::new(params);
        event_loop.run_app(&mut app).map_err(|e| e.to_string())?;
        if let Some(err) = app.take_error() {
            Err(err)
        } else {
            Ok(())
        }
    }
}

/// Run the helper window configured by the provided parameters.
#[allow(clippy::too_many_arguments)]
pub fn run_focus_winhelper(
    title: &str,
    time_ms: u64,
    delay_setframe_ms: u64,
    delay_apply_ms: u64,
    tween_ms: u64,
    apply_target: Option<(f64, f64, f64, f64)>,
    apply_grid: Option<(u32, u32, u32, u32)>,
    slot: Option<u8>,
    grid: Option<(u32, u32, u32, u32)>,
    size: Option<(f64, f64)>,
    pos: Option<(f64, f64)>,
    label_text: Option<String>,
    min_size: Option<(f64, f64)>,
    step_size: Option<(f64, f64)>,
    start_minimized: bool,
    start_zoomed: bool,
    panel_nonmovable: bool,
    panel_nonresizable: bool,
    attach_sheet: bool,
) -> Result<(), String> {
    let config = HelperConfig {
        time_ms,
        delay_setframe_ms,
        delay_apply_ms,
        tween_ms,
        apply_target,
        apply_grid,
        slot,
        grid,
        size,
        pos,
        label_text,
        min_size,
        step_size,
        start_minimized,
        start_zoomed,
        panel_nonmovable,
        panel_nonresizable,
        attach_sheet,
        ..HelperConfig::default()
    };
    run_helper_window(title.to_string(), config)
}

mod config {
    use std::time::Duration;

    #[derive(Clone, Copy)]
    pub(super) struct HelperWindowConfig {
        pub width_px: f64,
        pub height_px: f64,
        pub margin_px: f64,
    }

    #[derive(Clone, Copy)]
    pub(super) struct InputDelays {
        pub retry_delay_ms: u64,
        pub window_registration_delay_ms: u64,
    }

    #[derive(Clone, Copy)]
    pub(super) struct PlaceConfig {
        pub eps: f64,
    }

    pub(super) const HELPER_WINDOW: HelperWindowConfig = HelperWindowConfig {
        width_px: 280.0,
        height_px: 180.0,
        margin_px: 8.0,
    };

    pub(super) const INPUT_DELAYS: InputDelays = InputDelays {
        retry_delay_ms: 80,
        window_registration_delay_ms: 80,
    };

    pub(super) const PLACE: PlaceConfig = PlaceConfig { eps: 2.0 };

    pub(super) const fn ms(millis: u64) -> Duration {
        Duration::from_millis(millis)
    }
}

mod world {
    use std::{future::Future, sync::Arc, thread, time::Duration};

    use hotki_world_ids::WorldWindowId;
    use mac_winops::{self, WindowInfo, ops::RealWinOps};
    use once_cell::sync::OnceCell;
    use regex::Regex;
    use tokio::runtime::{Builder, Runtime};

    use crate::{
        CommandReceipt, PlaceAttemptOptions, RaiseIntent, World, WorldCfg, WorldView, WorldWindow,
    };

    type Result<T> = std::result::Result<T, String>;

    static RUNTIME: OnceCell<Runtime> = OnceCell::new();
    static WORLD: OnceCell<Arc<dyn WorldView>> = OnceCell::new();

    fn runtime() -> &'static Runtime {
        RUNTIME.get_or_init(|| {
            Builder::new_multi_thread()
                .enable_all()
                .worker_threads(2)
                .thread_name("mimic-rt")
                .build()
                .expect("failed to build mimic runtime")
        })
    }

    fn block_on<F>(fut: F) -> F::Output
    where
        F: Future,
    {
        runtime().block_on(fut)
    }

    fn ensure_world() -> Result<Arc<dyn WorldView>> {
        let rt = runtime();
        let guard = rt.enter();
        let world = WORLD
            .get_or_init(|| {
                let winops = Arc::new(RealWinOps);
                World::spawn_view(winops, WorldCfg::default())
            })
            .clone();
        drop(guard);
        Ok(world)
    }

    fn convert_window(w: WorldWindow) -> WindowInfo {
        WindowInfo {
            app: w.app,
            title: w.title,
            pid: w.pid,
            id: w.id,
            pos: w.pos,
            space: w.space,
            layer: w.layer,
            focused: w.focused,
            is_on_screen: w.is_on_screen,
            on_active_space: w.on_active_space,
        }
    }

    pub(super) fn list_windows() -> Result<Vec<WindowInfo>> {
        let world = ensure_world()?;
        let windows = block_on(async move { world.list_windows().await })
            .into_iter()
            .map(convert_window)
            .collect();
        Ok(windows)
    }

    pub(super) fn place_window(
        target: WorldWindowId,
        cols: u32,
        rows: u32,
        col: u32,
        row: u32,
        options: Option<PlaceAttemptOptions>,
    ) -> Result<CommandReceipt> {
        let world = ensure_world()?;
        let receipt = block_on(async move {
            world
                .request_place_for_window(target, cols, rows, col, row, options)
                .await
        })
        .map_err(|err| format!("world place_window failed: {err:?}"))?;
        mac_winops::drain_main_ops();
        Ok(receipt)
    }

    pub(super) fn ensure_frontmost(
        pid: i32,
        title: &str,
        attempts: usize,
        delay_ms: u64,
    ) -> Result<()> {
        let regex = Regex::new(&format!("^{}$", regex::escape(title)))
            .map_err(|e| format!("invalid title regex: {e}"))?;
        let intent = RaiseIntent {
            app_regex: None,
            title_regex: Some(Arc::new(regex)),
        };

        for attempt in 0..attempts {
            let world = ensure_world()?;
            let receipt = block_on(async { world.request_raise(intent.clone()).await })
                .map_err(|err| format!("world raise failed: {err:?}"))?;
            if let Some(target) = receipt.target
                && target.pid == pid
                && target.title == title
            {
                return Ok(());
            }
            if attempt + 1 < attempts {
                thread::sleep(Duration::from_millis(delay_ms));
            }
        }

        Err(format!(
            "failed to raise window pid={} title='{}' after {} attempts",
            pid, title, attempts
        ))
    }
}
