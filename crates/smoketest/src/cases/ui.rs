//! UI-driven smoketest cases executed via the registry runner.

use std::{
    fs,
    path::PathBuf,
    thread,
    time::{Duration, Instant},
};

use hotki_server::smoketest_bridge::BridgeEvent;
use hotki_world::WorldWindow;
use serde::Serialize;
use tracing::debug;

use crate::{
    config,
    error::{Error, Result},
    server_drive::{self, DriverError},
    session::{HotkiSession, HotkiSessionConfig},
    suite::{CaseCtx, sanitize_slug},
    world,
};

/// Window title emitted by the HUD process.
const HUD_TITLE: &str = "Hotki HUD";
/// Canonical activation chord identifier expected from the server.
pub const ACTIVATION_IDENT: &str = "shift+cmd+0";

/// Result payload describing an activation attempt.
#[derive(Debug, Clone)]
pub struct ActivationOutcome {
    /// Time (ms) from activation send until the HUD was observed on-screen.
    hud_update_ms: Option<u64>,
    /// Time (ms) from activation send until the HUD was confirmed frontmost.
    frontmost_ms: Option<u64>,
    /// Number of activation chord injections issued while waiting.
    activation_attempts: u32,
    /// How many HUD update observations were seen in total.
    hud_updates_seen: u32,
}

impl ActivationOutcome {
    /// Earliest timing when the HUD was confirmed visible.
    fn hud_visible_ms(&self) -> Option<u64> {
        self.hud_update_ms.or(self.frontmost_ms)
    }

    /// Whether the HUD visibility was confirmed via polling observations.
    fn observed_via_event(&self) -> bool {
        self.hud_update_ms.is_some()
    }

    /// Render a concise summary string for logging.
    fn summary_string(&self) -> String {
        format!(
            "hud_update_ms={:?} frontmost_ms={:?} activation_attempts={} hud_updates_seen={} observed_via_event={}",
            self.hud_update_ms,
            self.frontmost_ms,
            self.activation_attempts,
            self.hud_updates_seen,
            self.observed_via_event()
        )
    }
}

/// Observes HUD visibility while waiting for activation to settle.
struct BindingWatcher {
    /// PID of the HUD window used to verify visibility.
    hud_pid: i32,
}

impl BindingWatcher {
    /// Create a watcher scoped to the target HUD process id.
    fn new(hud_pid: i32) -> Self {
        Self { hud_pid }
    }

    /// Attempt to activate the HUD and wait until the expected submenu bindings appear.
    fn activate_until_ready(
        &self,
        activation_ident: &str,
        expected_nested: &[&str],
        timeout_ms: u64,
    ) -> Result<ActivationOutcome> {
        let start = Instant::now();
        let mut metrics = ActivationMetrics::new(start);
        let deadline = start + Duration::from_millis(timeout_ms);
        let poll_interval = Duration::from_millis(config::INPUT_DELAYS.poll_interval_ms.max(10));
        let mut last_attempt: Option<Instant> = None;
        let mut last_hud_event: Option<u64> = None;

        self.dispatch_activation(
            activation_ident,
            deadline,
            timeout_ms,
            &mut metrics,
            &mut last_attempt,
        )?;

        while Instant::now() < deadline {
            let mut hud_ready_via_event = false;
            match server_drive::drain_bridge_events() {
                Ok(events) => {
                    for event in events {
                        debug!(
                            event_id = event.id,
                            event_ms = event.timestamp_ms,
                            "hud_event_observed"
                        );
                        if let BridgeEvent::Hud { .. } = event.payload
                            && last_hud_event != Some(event.id)
                        {
                            metrics.record_hud_update();
                            metrics.record_frontmost();
                            last_hud_event = Some(event.id);
                            if let Ok(Some(snapshot)) = server_drive::latest_hud() {
                                debug!(
                                    hud_event_id = snapshot.event_id,
                                    depth = snapshot.depth,
                                    parent = ?snapshot.parent_title,
                                    received_ms = snapshot.received_ms,
                                    viewing_root = snapshot.cursor.viewing_root,
                                    key_count = snapshot.keys.len(),
                                    "hud_snapshot_state"
                                );
                            }
                            if expected_nested.is_empty() {
                                hud_ready_via_event = true;
                            }
                        }
                    }
                }
                Err(DriverError::NotInitialized) => {}
                Err(err) => return Err(Error::from(err)),
            }

            if hud_ready_via_event {
                return Ok(metrics.into_outcome());
            }

            if self.hud_visible()? {
                metrics.record_hud_update();
                metrics.record_frontmost();

                if expected_nested.is_empty() {
                    return Ok(metrics.into_outcome());
                }

                if let Some(remaining_ms) = remaining_ms(deadline) {
                    let nested_timeout = remaining_ms.min(config::BINDING_GATES.default_ms);
                    match server_drive::wait_for_idents(expected_nested, nested_timeout) {
                        Ok(()) => return Ok(metrics.into_outcome()),
                        Err(server_drive::DriverError::BindingTimeout { .. }) => {
                            // Nested bindings not ready yet; retry activation below.
                            last_attempt = None;
                        }
                        Err(err) => return Err(Error::from(err)),
                    }
                } else {
                    break;
                }
            }

            if should_retry(last_attempt) {
                self.dispatch_activation(
                    activation_ident,
                    deadline,
                    timeout_ms,
                    &mut metrics,
                    &mut last_attempt,
                )?;
            }

            thread::sleep(poll_interval);
        }

        Err(Error::HudNotVisible { timeout_ms })
    }

    /// Attempt to inject the activation chord if enough time remains before the deadline.
    fn dispatch_activation(
        &self,
        activation_ident: &str,
        deadline: Instant,
        total_timeout_ms: u64,
        metrics: &mut ActivationMetrics,
        last_attempt: &mut Option<Instant>,
    ) -> Result<()> {
        let remaining = remaining_ms(deadline).ok_or(Error::HudNotVisible {
            timeout_ms: total_timeout_ms,
        })?;
        server_drive::wait_for_idents(&[activation_ident], remaining)?;
        server_drive::inject_key(activation_ident)?;
        metrics.record_activation();
        *last_attempt = Some(Instant::now());
        Ok(())
    }

    /// Check whether the HUD window is visible on the active space.
    fn hud_visible(&self) -> Result<bool> {
        let windows = world::list_windows()?;
        Ok(windows
            .iter()
            .any(|w| w.pid == self.hud_pid && w.title == HUD_TITLE && w.is_on_screen))
    }
}

/// Internal activation timing accumulator.
#[derive(Debug)]
struct ActivationMetrics {
    /// Activation start instant used as the timing baseline.
    start: Instant,
    /// Instant captured when the HUD was first observed on-screen.
    hud_update_at: Option<Instant>,
    /// Instant captured when the HUD was first confirmed frontmost.
    frontmost_at: Option<Instant>,
    /// Number of activation chords injected while waiting.
    activation_attempts: u32,
    /// Count of HUD observations recorded during activation.
    hud_updates_seen: u32,
}

impl ActivationMetrics {
    /// Create a fresh metrics accumulator anchored at `start`.
    fn new(start: Instant) -> Self {
        Self {
            start,
            hud_update_at: None,
            frontmost_at: None,
            activation_attempts: 0,
            hud_updates_seen: 0,
        }
    }

    /// Record that an activation chord was injected.
    fn record_activation(&mut self) {
        self.activation_attempts = self.activation_attempts.saturating_add(1);
    }

    /// Record that the HUD was observed on-screen while waiting.
    fn record_hud_update(&mut self) {
        self.hud_updates_seen = self.hud_updates_seen.saturating_add(1);
        if self.hud_update_at.is_none() {
            self.hud_update_at = Some(Instant::now());
        }
    }

    /// Record that the HUD was confirmed as the frontmost window.
    fn record_frontmost(&mut self) {
        if self.frontmost_at.is_none() {
            self.frontmost_at = Some(Instant::now());
        }
    }

    /// Convert accumulated timing data into a public-facing outcome.
    fn into_outcome(self) -> ActivationOutcome {
        let hud_update_ms = self
            .hud_update_at
            .map(|ts| ts.saturating_duration_since(self.start).as_millis() as u64);
        let frontmost_ms = self
            .frontmost_at
            .map(|ts| ts.saturating_duration_since(self.start).as_millis() as u64);
        ActivationOutcome {
            hud_update_ms,
            frontmost_ms,
            activation_attempts: self.activation_attempts,
            hud_updates_seen: self.hud_updates_seen,
        }
    }
}

/// Determine whether another activation attempt should be issued.
fn should_retry(last_attempt: Option<Instant>) -> bool {
    match last_attempt {
        None => true,
        Some(ts) => {
            Instant::now().saturating_duration_since(ts)
                >= config::ms(config::INPUT_DELAYS.retry_delay_ms)
        }
    }
}

/// Compute milliseconds remaining before the provided deadline.
fn remaining_ms(deadline: Instant) -> Option<u64> {
    deadline
        .checked_duration_since(Instant::now())
        .and_then(|dur| {
            let ms = dur.as_millis() as u64;
            if ms > 0 { Some(ms) } else { None }
        })
}

/// Key sequence applied once the HUD is visible to exercise the demo flow.
const UI_DEMO_SEQUENCE: &[&str] = &["t", "l", "l", "l", "l", "l", "esc"];

/// Standard HUD demo configuration (full HUD mode anchored bottom-right).
const UI_DEMO_CONFIG: &str = r#"(
    keys: [
        ("shift+cmd+0", "activate", keys([
            ("t", "Theme tester", keys([
                ("h", "Theme Prev", theme_prev, (noexit: true)),
                ("l", "Theme Next", theme_next, (noexit: true)),
            ])),
        ])),
        ("shift+cmd+0", "exit", exit, (global: true, hide: true)),
        ("esc", "Back", pop, (global: true, hide: true, hud_only: true)),
    ],
    style: (hud: (mode: hud, pos: se)),
)"#;

/// Mini HUD demo configuration that mirrors the standard flow in mini mode.
const MINUI_DEMO_CONFIG: &str = r#"(
    keys: [
        ("shift+cmd+0", "activate", keys([
            ("t", "Theme tester", keys([
                ("h", "Theme Prev", theme_prev, (noexit: true)),
                ("l", "Theme Next", theme_next, (noexit: true)),
            ])),
        ])),
        ("shift+cmd+0", "exit", exit, (global: true, hide: true)),
        ("esc", "Back", pop, (global: true, hide: true, hud_only: true)),
    ],
    style: (hud: (mode: mini, pos: se)),
)"#;

/// Execute the standard HUD demo flow using the registry runner.
pub fn ui_demo_standard(ctx: &mut CaseCtx<'_>) -> Result<()> {
    run_ui_case(
        ctx,
        &UiCaseSpec {
            slug: "ui.demo.standard",
            ron_config: UI_DEMO_CONFIG,
            with_logs: true,
        },
    )
}

/// Execute the mini HUD demo flow using the registry runner.
pub fn ui_demo_mini(ctx: &mut CaseCtx<'_>) -> Result<()> {
    run_ui_case(
        ctx,
        &UiCaseSpec {
            slug: "ui.demo.mini",
            ron_config: MINUI_DEMO_CONFIG,
            with_logs: false,
        },
    )
}

/// Parameters describing a UI demo scenario.
struct UiCaseSpec {
    /// Registry slug recorded in artifacts.
    slug: &'static str,
    /// RON configuration injected into the hotki session.
    ron_config: &'static str,
    /// Whether to enable verbose logging for the child process.
    with_logs: bool,
}

/// Mutable state shared across UI case stages.
/// State retained for each HUD demo invocation.
struct UiCaseState {
    /// Running hotki session launched for this demo.
    session: HotkiSession,
    /// Registry slug mirrored when logging diagnostics or creating scratch configs.
    slug: &'static str,
    /// Filesystem path to the RON configuration applied for this demo.
    config_path: PathBuf,
    /// Observed time in milliseconds until the HUD became visible.
    time_to_hud_ms: Option<u64>,
    /// Snapshot of HUD-related windows after activation.
    hud_windows: Option<Vec<HudWindowSnapshot>>,
    /// Detailed activation diagnostics for the HUD.
    hud_activation: Option<ActivationOutcome>,
}

/// Core implementation that runs a HUD-focused UI smoketest.
fn run_ui_case(ctx: &mut CaseCtx<'_>, spec: &UiCaseSpec) -> Result<()> {
    let mut state: Option<UiCaseState> = None;

    ctx.setup(|ctx| {
        let filename = format!("{}_config.ron", sanitize_slug(spec.slug));
        let config_path = ctx.scratch_path(filename);
        fs::write(&config_path, spec.ron_config.as_bytes())?;

        let session = HotkiSession::spawn(
            HotkiSessionConfig::from_env()?
                .with_config(&config_path)
                .with_logs(spec.with_logs),
        )?;

        state = Some(UiCaseState {
            session,
            slug: spec.slug,
            config_path,
            time_to_hud_ms: None,
            hud_windows: None,
            hud_activation: None,
        });

        Ok(())
    })?;

    ctx.action(|_| {
        let state_ref = state
            .as_mut()
            .ok_or_else(|| Error::InvalidState("ui case state missing during action".into()))?;

        let socket = state_ref.session.socket_path().to_string();
        server_drive::ensure_init(&socket, 3_000)?;
        server_drive::set_config_from_path(&state_ref.config_path)?;
        let gate_ms = config::BINDING_GATES.default_ms * 5;
        let watcher = BindingWatcher::new(state_ref.session.pid() as i32);
        let ident_activate = UI_DEMO_SEQUENCE[0];
        let activation =
            watcher.activate_until_ready(ACTIVATION_IDENT, &[ident_activate], gate_ms)?;

        server_drive::inject_key(ident_activate)?;
        server_drive::wait_for_idents(&["h", "l"], gate_ms)?;

        let theme_steps = &UI_DEMO_SEQUENCE[1..UI_DEMO_SEQUENCE.len() - 1];
        server_drive::inject_sequence(theme_steps)?;

        let hud_windows = collect_hud_windows(state_ref.session.pid() as i32)?;
        if hud_windows.is_empty() {
            return Err(Error::InvalidState(
                "hud window not visible after activation".into(),
            ));
        }

        state_ref.time_to_hud_ms = activation.hud_visible_ms();
        state_ref.hud_activation = Some(activation);
        state_ref.hud_windows = Some(hud_windows);

        // Close the HUD after capturing window diagnostics to leave the session cleanly.
        let ident_exit = UI_DEMO_SEQUENCE[UI_DEMO_SEQUENCE.len() - 1];
        server_drive::inject_key(ident_exit)?;
        Ok(())
    })?;

    ctx.settle(|ctx| {
        let mut state_inner = state
            .take()
            .ok_or_else(|| Error::InvalidState("ui case state missing during settle".into()))?;

        let hud_windows_desc = state_inner
            .hud_windows
            .as_ref()
            .map(|wins| {
                wins.iter()
                    .map(|w| {
                        format!(
                            "title={} pid={} id={} on_active={} on_screen={} layer={}",
                            w.title, w.pid, w.id, w.on_active_space, w.is_on_screen, w.layer
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("; ")
            })
            .unwrap_or_else(|| "<none>".to_string());
        let hud_activation_summary = state_inner
            .hud_activation
            .as_ref()
            .map_or_else(|| "<none>".to_string(), ActivationOutcome::summary_string);
        ctx.log_event(
            "ui_demo_outcome",
            &format!(
                "slug={} hud_seen={} time_to_hud_ms={:?} hud_windows={} hud_activation={}",
                state_inner.slug,
                state_inner.time_to_hud_ms.is_some(),
                state_inner.time_to_hud_ms,
                hud_windows_desc,
                hud_activation_summary
            ),
        );

        state_inner.session.shutdown();
        state_inner.session.kill_and_wait();
        server_drive::reset();

        Ok(())
    })?;

    Ok(())
}

/// Collect HUD windows belonging to the active Hotki session.
fn collect_hud_windows(pid: i32) -> Result<Vec<HudWindowSnapshot>> {
    let windows = world::list_windows()?;
    Ok(windows
        .into_iter()
        .filter(|w| w.pid == pid && w.title == "Hotki HUD")
        .map(HudWindowSnapshot::from)
        .collect())
}

/// Serializable summary of a HUD window observation.
#[derive(Serialize)]
struct HudWindowSnapshot {
    /// Owning process identifier.
    pid: i32,
    /// Window identifier within the process.
    id: u32,
    /// Observed window title.
    title: String,
    /// Whether the window was on the active space.
    on_active_space: bool,
    /// Whether the window was reported on-screen.
    is_on_screen: bool,
    /// Window layer as reported by CoreGraphics.
    layer: i32,
}

impl From<WorldWindow> for HudWindowSnapshot {
    fn from(info: WorldWindow) -> Self {
        Self {
            pid: info.pid,
            id: info.id,
            title: info.title,
            on_active_space: info.on_active_space,
            is_on_screen: info.is_on_screen,
            layer: info.layer,
        }
    }
}
