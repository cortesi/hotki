use std::{
    cell::RefCell,
    rc::{Rc, Weak},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use tracing::{debug, warn};
use winit::platform::pump_events::PumpStatus;

use super::{
    event_loop::{EventLoopHandle, shared_event_loop},
    helper_app,
    registry::{purge_slug, register_mimic},
    scenario::{MimicScenario, Quirk, apply_quirk_defaults, format_quirks},
};
use crate::PlaceOptions;

/// Internal registry mapping spawned mimic windows to diagnostic metadata so world reconciliation
/// can surface `{scenario_slug, window_label, quirks[]}`.
static ACTIVE_MIMIC_COUNT: AtomicUsize = AtomicUsize::new(0);

thread_local! {
    static ACTIVE_MIMICS: RefCell<Vec<Weak<MimicRuntime>>> = const { RefCell::new(Vec::new()) };
}

fn register_active_mimic(runtime: &Rc<MimicRuntime>) {
    ACTIVE_MIMICS.with(|list| list.borrow_mut().push(Rc::downgrade(runtime)));
    ACTIVE_MIMIC_COUNT.fetch_add(1, Ordering::SeqCst);
}

/// Pump all active mimic runtimes once. Call from main-thread wait loops.
pub fn pump_active_mimics() {
    ACTIVE_MIMICS.with(|list| {
        let mut entries = list.borrow_mut();
        entries.retain(|weak| {
            if let Some(runtime) = weak.upgrade() {
                runtime.pump();
                if runtime.is_finished() {
                    ACTIVE_MIMIC_COUNT.fetch_sub(1, Ordering::SeqCst);
                    false
                } else {
                    true
                }
            } else {
                ACTIVE_MIMIC_COUNT.fetch_sub(1, Ordering::SeqCst);
                false
            }
        });
    });
}

/// Return the number of active mimic runtimes.
#[must_use]
pub fn active_count() -> usize {
    ACTIVE_MIMIC_COUNT.load(Ordering::SeqCst)
}

/// Request shutdown for all active mimics, returning the number signalled.
pub fn request_shutdown_all() -> usize {
    ACTIVE_MIMICS.with(|list| {
        let mut requested = 0;
        for weak in list.borrow().iter() {
            if let Some(runtime) = weak.upgrade() {
                runtime.request_shutdown();
                requested += 1;
            }
        }
        requested
    })
}

/// Pump active mimics until the deadline elapses or none remain.
pub fn wait_until_idle(deadline: Instant) -> bool {
    while Instant::now() < deadline {
        pump_active_mimics();
        if active_count() == 0 {
            return true;
        }
        thread::sleep(Duration::from_millis(5));
    }
    pump_active_mimics();
    active_count() == 0
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

struct MimicRuntime {
    event_loop: EventLoopHandle,
    app: RefCell<helper_app::HelperApp>,
    shutdown: Arc<AtomicBool>,
    finished: RefCell<bool>,
    result: RefCell<Option<Result<(), String>>>,
}

impl MimicRuntime {
    fn new(
        event_loop: EventLoopHandle,
        app: helper_app::HelperApp,
        shutdown: Arc<AtomicBool>,
    ) -> Self {
        Self {
            event_loop,
            app: RefCell::new(app),
            shutdown,
            finished: RefCell::new(false),
            result: RefCell::new(None),
        }
    }

    fn request_shutdown(&self) {
        self.shutdown.store(true, Ordering::SeqCst);
    }

    fn pump(&self) {
        if *self.finished.borrow() {
            return;
        }
        let timeout = {
            let app = self.app.borrow();
            app.next_wakeup_timeout()
        };
        let status = {
            let mut app = self.app.borrow_mut();
            self.event_loop.pump_app(Some(timeout), &mut *app)
        };
        match status {
            PumpStatus::Continue => {
                let should_finish = {
                    let app = self.app.borrow();
                    app.should_finish()
                };
                if should_finish {
                    self.finish_with(0);
                }
            }
            PumpStatus::Exit(code) => {
                self.finish_with(code);
            }
        }
    }

    fn finish_with(&self, code: i32) {
        if *self.finished.borrow() {
            return;
        }
        *self.finished.borrow_mut() = true;
        let error = self.app.borrow_mut().take_error();
        let result = match (code, error) {
            (_, Some(err)) => Err(err),
            (0, None) => Ok(()),
            (status, None) => Err(format!("mimic exited with status {status}")),
        };
        *self.result.borrow_mut() = Some(result);
    }

    fn is_finished(&self) -> bool {
        *self.finished.borrow()
    }

    fn take_result(&self) -> Option<Result<(), String>> {
        self.result.borrow_mut().take()
    }
}

struct MimicWindowHandle {
    label: Arc<str>,
    quirks: Vec<Quirk>,
    place: PlaceOptions,
    runtime: Rc<MimicRuntime>,
}

impl MimicWindowHandle {
    fn shutdown(&self) {
        self.runtime.request_shutdown();
    }

    fn join(&self) -> Result<(), MimicError> {
        loop {
            self.runtime.pump();
            if self.runtime.is_finished() {
                if let Some(result) = self.runtime.take_result() {
                    return result
                        .map_err(|e| MimicError::HelperFailure(self.label.to_string(), e));
                }
                return Ok(());
            }
            pump_active_mimics();
            thread::sleep(Duration::from_millis(1));
        }
    }
}

/// Errors surfaced by mimic helper management.
#[derive(Debug)]
pub enum MimicError {
    /// Helper reported a recoverable failure.
    HelperFailure(String, String),
    /// Failed to initialize the helper runtime.
    SpawnFailed {
        /// Label identifying the helper window that failed to start.
        label: String,
        /// Reason reported by the underlying windowing layer.
        reason: String,
    },
}

impl std::fmt::Display for MimicError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::HelperFailure(label, err) => {
                write!(f, "mimic window '{label}' failed: {err}")
            }
            Self::SpawnFailed { label, reason } => {
                write!(f, "failed to spawn mimic window '{label}': {reason}")
            }
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

        let event_loop = shared_event_loop();
        debug!(slug = %slug, label = %spec.window_label, "helper_app_construct_begin");
        let app = helper_app::HelperApp::new(helper_app::HelperParams::from_config(
            decorated_title,
            helper_config,
        ));
        debug!(slug = %slug, label = %spec.window_label, "helper_app_construct_end");
        let runtime = Rc::new(MimicRuntime::new(event_loop.clone(), app, shutdown.clone()));
        register_active_mimic(&runtime);
        debug!(slug = %slug, label = %spec.window_label, "helper_runtime_registered");

        register_mimic(slug.clone(), label.clone(), quirks.clone(), place);
        debug!(slug = %slug, label = %spec.window_label, "helper_registry_registered");

        handles.push(MimicWindowHandle {
            label,
            quirks,
            place,
            runtime: runtime.clone(),
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
    purge_slug(&handle.slug);
    Ok(())
}
