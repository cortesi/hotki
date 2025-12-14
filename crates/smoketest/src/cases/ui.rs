//! UI-driven smoketest cases executed via the registry runner.

use std::{
    cmp::Ordering,
    fs,
    path::PathBuf,
    thread,
    time::{Duration, Instant},
};

use core_foundation::{
    array::CFArray,
    base::{CFType, ItemRef, TCFType},
    dictionary::CFDictionary,
    number::CFNumber,
    string::CFString,
};
use core_graphics::{
    display::CGDisplay,
    geometry::{CGPoint, CGRect, CGSize},
    window::{
        copy_window_info, kCGNullWindowID, kCGWindowBounds, kCGWindowIsOnscreen, kCGWindowLayer,
        kCGWindowListExcludeDesktopElements, kCGWindowListOptionOnScreenOnly, kCGWindowName,
        kCGWindowNumber, kCGWindowOwnerName, kCGWindowOwnerPID,
    },
};
use serde::Serialize;
use tracing::{debug, info};

use crate::{
    config,
    error::{Error, Result},
    server_drive::{BridgeDriver, BridgeEvent, DriverError},
    session::{HotkiSession, HotkiSessionConfig},
    suite::{CaseCtx, LOG_TARGET, sanitize_slug},
};

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
    /// Whether a focus-change event was observed during activation.
    focus_event_seen: bool,
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
            "hud_update_ms={:?} frontmost_ms={:?} activation_attempts={} hud_updates_seen={} observed_via_event={} focus_event_seen={}",
            self.hud_update_ms,
            self.frontmost_ms,
            self.activation_attempts,
            self.hud_updates_seen,
            self.observed_via_event(),
            self.focus_event_seen
        )
    }
}

/// Observes HUD visibility while waiting for activation to settle.
struct BindingWatcher;

impl BindingWatcher {
    /// Create a watcher scoped to the target HUD process id.
    fn new(_hud_pid: i32) -> Self {
        Self
    }

    /// Attempt to activate the HUD and wait until the expected submenu bindings appear.
    fn activate_until_ready(
        &self,
        bridge: &mut BridgeDriver,
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
            bridge,
            activation_ident,
            deadline,
            timeout_ms,
            &mut metrics,
            &mut last_attempt,
        )?;

        while Instant::now() < deadline {
            let mut hud_ready_via_event = false;
            match bridge.drain_bridge_events() {
                Ok(events) => {
                    for event in events {
                        match event.payload {
                            BridgeEvent::Hud {
                                ref cursor,
                                depth,
                                ref parent_title,
                                ..
                            } => {
                                metrics.focus_event_seen |= cursor.app.is_some();
                                debug!(
                                    event_id = event.id,
                                    event_ms = event.timestamp_ms,
                                    depth,
                                    parent = ?parent_title,
                                    viewing_root = cursor.viewing_root,
                                    "hud_event_observed"
                                );
                                if last_hud_event == Some(event.id) {
                                    continue;
                                }
                                metrics.record_hud_update();
                                metrics.record_frontmost();
                                last_hud_event = Some(event.id);
                                if let Ok(Some(snapshot)) = bridge.latest_hud() {
                                    metrics.focus_event_seen |= snapshot.cursor.app.is_some();
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
                            BridgeEvent::Focus { .. } => {
                                metrics.focus_event_seen = true;
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

            if self.hud_visible(bridge)? {
                metrics.record_hud_update();
                metrics.record_frontmost();

                if expected_nested.is_empty() {
                    return Ok(metrics.into_outcome());
                }

                if let Some(remaining_ms) = remaining_ms(deadline) {
                    let nested_timeout = remaining_ms.min(config::BINDING_GATES.default_ms);
                    match bridge.wait_for_idents(expected_nested, nested_timeout) {
                        Ok(()) => return Ok(metrics.into_outcome()),
                        Err(DriverError::BindingTimeout { .. }) => {
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
                    bridge,
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
        bridge: &mut BridgeDriver,
        activation_ident: &str,
        deadline: Instant,
        total_timeout_ms: u64,
        metrics: &mut ActivationMetrics,
        last_attempt: &mut Option<Instant>,
    ) -> Result<()> {
        let remaining = remaining_ms(deadline).ok_or(Error::HudNotVisible {
            timeout_ms: total_timeout_ms,
        })?;
        bridge.wait_for_idents(&[activation_ident], remaining)?;
        bridge.inject_key(activation_ident)?;
        metrics.record_activation();
        *last_attempt = Some(Instant::now());
        Ok(())
    }

    /// Check whether the HUD window is visible on the active space.
    fn hud_visible(&self, bridge: &BridgeDriver) -> Result<bool> {
        Ok(bridge.latest_hud()?.is_some())
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
    /// Whether a focus-change event was observed.
    focus_event_seen: bool,
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
            focus_event_seen: false,
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
            focus_event_seen: self.focus_event_seen,
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
const UI_DEMO_SEQUENCE: &[&str] = &["t", "l", "l", "l", "l", "l", "n", "esc"];

/// Standard HUD demo configuration (full HUD mode anchored bottom-right).
const UI_DEMO_CONFIG: &str = r#"
style(#{
  hud: #{
    mode: hud_full,
    pos: se,
  },
});

global.mode("shift+cmd+0", "activate", |m| {
  m.mode("t", "Theme tester", |sub| {
    sub.bind("h", "Theme Prev", action.theme_prev).no_exit();
    sub.bind("l", "Theme Next", action.theme_next).no_exit();
    sub.bind("n", "Notify", action.shell("echo notify").notify(info, warn)).no_exit();
  });
});

global.bind("esc", "Back", action.pop).global().hidden().hud_only();
"#;

/// Mini HUD demo configuration.
const MINI_HUD_CONFIG: &str = r#"
style(#{
  hud: #{
    mode: hud_mini,
    pos: se,
  },
});

global.mode("shift+cmd+0", "activate", |m| {
  m.mode("t", "Theme tester", |sub| {
    sub.bind("h", "Theme Prev", action.theme_prev).no_exit();
    sub.bind("l", "Theme Next", action.theme_next).no_exit();
    sub.bind("n", "Notify", action.shell("echo notify").notify(info, warn)).no_exit();
  });
});

global.bind("esc", "Back", action.pop).global().hidden().hud_only();
"#;

/// Verify full HUD appears and responds to keys.
pub fn hud(ctx: &mut CaseCtx<'_>) -> Result<()> {
    run_ui_case(
        ctx,
        &UiCaseSpec {
            slug: "hud",
            rhai_config: UI_DEMO_CONFIG,
            with_logs: true,
        },
    )
}

/// Verify mini HUD appears and responds to keys.
pub fn mini(ctx: &mut CaseCtx<'_>) -> Result<()> {
    run_ui_case(
        ctx,
        &UiCaseSpec {
            slug: "mini",
            rhai_config: MINI_HUD_CONFIG,
            with_logs: true,
        },
    )
}

/// Verify HUD placement on multi-display setups.
pub fn displays(ctx: &mut CaseCtx<'_>) -> Result<()> {
    run_ui_case_with(
        ctx,
        &UiCaseSpec {
            slug: "displays",
            rhai_config: UI_DEMO_CONFIG,
            with_logs: true,
        },
        verify_display_alignment,
    )
}

/// Parameters describing a UI demo scenario.
struct UiCaseSpec {
    /// Registry slug recorded in artifacts.
    slug: &'static str,
    /// Rhai configuration injected into the hotki session.
    rhai_config: &'static str,
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
    /// Filesystem path to the Rhai configuration applied for this demo.
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
    run_ui_case_with(ctx, spec, |_| Ok(()))
}

/// Variant of `run_ui_case` that allows callers to inject extra checks after HUD activation.
fn run_ui_case_with<F>(ctx: &mut CaseCtx<'_>, spec: &UiCaseSpec, after_activation: F) -> Result<()>
where
    F: Fn(&mut UiCaseState) -> Result<()>,
{
    let mut state: Option<UiCaseState> = None;

    ctx.setup(|ctx| {
        let filename = format!("{}_config.rhai", sanitize_slug(spec.slug));
        let config_path = ctx.scratch_path(filename);
        fs::write(&config_path, spec.rhai_config.as_bytes())?;

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

        {
            let bridge = state_ref.session.bridge_mut();
            bridge.ensure_ready(1_000)?;
            bridge.set_config_from_path(&state_ref.config_path)?;
        }
        let gate_ms = config::BINDING_GATES.default_ms * 2;
        let watcher = BindingWatcher::new(state_ref.session.pid() as i32);
        let ident_activate = UI_DEMO_SEQUENCE[0];
        let activation = {
            let bridge = state_ref.session.bridge_mut();
            watcher.activate_until_ready(bridge, ACTIVATION_IDENT, &[ident_activate], gate_ms)?
        };
        if !activation.focus_event_seen {
            return Err(Error::InvalidState(
                "no focus-change bridge events observed during activation".into(),
            ));
        }

        {
            let bridge = state_ref.session.bridge_mut();
            bridge.inject_key(ident_activate)?;
            bridge.wait_for_idents(&["h", "l", "n"], gate_ms)?;
        }

        // Cycle through themes with visible delays
        let theme_steps = &UI_DEMO_SEQUENCE[1..UI_DEMO_SEQUENCE.len() - 1];
        {
            let bridge = state_ref.session.bridge_mut();
            for key in theme_steps.iter() {
                bridge.inject_key(key)?;
                thread::sleep(Duration::from_millis(config::THEME_SWITCH_DELAY_MS));
            }
        }

        let hud_windows = collect_hud_windows(state_ref.session.pid() as i32)?;

        state_ref.time_to_hud_ms = activation.hud_visible_ms();
        state_ref.hud_activation = Some(activation);
        state_ref.hud_windows = Some(hud_windows);

        after_activation(state_ref)?;

        // Close the HUD after capturing window diagnostics to leave the session cleanly.
        let ident_exit = UI_DEMO_SEQUENCE[UI_DEMO_SEQUENCE.len() - 1];
        {
            let bridge = state_ref.session.bridge_mut();
            bridge.inject_key(ident_exit)?;
        }
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
                            "title={} pid={} id={} display_id={:?} on_screen={} layer={}",
                            w.title, w.pid, w.id, w.display_id, w.is_on_screen, w.layer
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

        // Shutdown may fail if the server closes the socket before responding;
        // this is expected behavior, so we just log and continue.
        if let Err(err) = state_inner.session.shutdown() {
            tracing::debug!(
                ?err,
                "bridge shutdown returned error (expected on clean exit)"
            );
        }
        state_inner.session.kill_and_wait();

        Ok(())
    })?;

    Ok(())
}

/// Collect HUD windows belonging to the active Hotki session.
fn collect_hud_windows(pid: i32) -> Result<Vec<HudWindowSnapshot>> {
    let displays = enumerate_displays()?;
    let options = kCGWindowListOptionOnScreenOnly | kCGWindowListExcludeDesktopElements;
    let arr: CFArray = copy_window_info(options, kCGNullWindowID)
        .ok_or_else(|| Error::InvalidState("failed to read window list".into()))?;

    let key_layer = unsafe { CFString::wrap_under_get_rule(kCGWindowLayer) };
    let key_owner_pid = unsafe { CFString::wrap_under_get_rule(kCGWindowOwnerPID) };
    let key_owner_name = unsafe { CFString::wrap_under_get_rule(kCGWindowOwnerName) };
    let key_name = unsafe { CFString::wrap_under_get_rule(kCGWindowName) };
    let key_number = unsafe { CFString::wrap_under_get_rule(kCGWindowNumber) };
    let key_bounds = unsafe { CFString::wrap_under_get_rule(kCGWindowBounds) };
    let key_onscreen = unsafe { CFString::wrap_under_get_rule(kCGWindowIsOnscreen) };

    let mut windows = Vec::new();
    for raw in arr.iter() {
        let dict_ptr = *raw;
        let dict: CFDictionary<CFString, CFType> =
            unsafe { CFDictionary::wrap_under_get_rule(dict_ptr as _) };

        if dict_value_i32(&dict, &key_owner_pid) != Some(pid) {
            continue;
        }

        let title = dict_value_string(&dict, &key_name).unwrap_or_default();
        let id = dict_value_u32(&dict, &key_number).unwrap_or(0);
        let layer = dict_value_i32(&dict, &key_layer).unwrap_or(-1);
        let is_on_screen = dict_value_bool(&dict, &key_onscreen).unwrap_or(false);
        let display_id = dict_value_rect(&dict, &key_bounds)
            .as_ref()
            .and_then(|rect| display_for_rect(rect, &displays));

        windows.push(HudWindowSnapshot {
            pid,
            id,
            title: if title.is_empty() {
                dict_value_string(&dict, &key_owner_name).unwrap_or_default()
            } else {
                title
            },
            layer,
            is_on_screen,
            display_id,
        });
    }

    Ok(windows)
}

/// Serializable summary of a HUD window observation.
/// Serializable summary of a HUD window observation.
#[derive(Clone, Serialize)]
struct HudWindowSnapshot {
    /// Owning process identifier.
    pid: i32,
    /// Window identifier within the process.
    id: u32,
    /// Observed window title.
    title: String,
    /// Window layer as reported by CoreGraphics.
    layer: i32,
    /// Whether the window was reported on-screen.
    is_on_screen: bool,
    /// Display identifier derived from window bounds, when available.
    display_id: Option<u32>,
}

/// Verify the HUD aligns with the active display reported by the world service.
fn verify_display_alignment(state: &mut UiCaseState) -> Result<()> {
    let displays = enumerate_displays()?;
    if displays.len() < 2 {
        info!(
            target: LOG_TARGET,
            pid = state.session.pid(),
            count = displays.len(),
            "skipping display mapping check: fewer than two displays",
        );
        return Ok(());
    }

    let focused_display = focused_display_id(&displays).ok_or_else(|| {
        Error::InvalidState("unable to resolve focused display for verification".into())
    })?;

    let hud_snapshot =
        state.session.bridge_mut().latest_hud()?.ok_or_else(|| {
            Error::InvalidState("no HUD snapshot observed after activation".into())
        })?;
    let hud_active = hud_snapshot
        .displays
        .active
        .as_ref()
        .map(|d| d.id)
        .ok_or_else(|| Error::InvalidState("HUD snapshot missing active display".into()))?;

    if hud_active != focused_display {
        return Err(Error::InvalidState(format!(
            "active display mismatch: hud={} focused={}",
            hud_active, focused_display
        )));
    }

    let hud_windows = if let Some(wins) = state.hud_windows.clone() {
        wins
    } else {
        let wins = collect_hud_windows(state.session.pid() as i32)?;
        state.hud_windows = Some(wins.clone());
        wins
    };

    let mut mapped: Vec<u32> = hud_windows.iter().filter_map(|w| w.display_id).collect();
    if mapped.is_empty() {
        return Err(Error::InvalidState(
            "HUD windows missing display identifiers".into(),
        ));
    }
    mapped.sort_unstable();
    mapped.dedup();
    if mapped.len() != 1 || mapped[0] != hud_active {
        return Err(Error::InvalidState(format!(
            "HUD windows on displays {:?} (expected {})",
            mapped, hud_active
        )));
    }

    Ok(())
}

/// Lightweight description of a display's bounds in bottom-left coordinates.
#[derive(Clone, Copy, Debug)]
struct DisplayFrame {
    /// Display identifier.
    id: u32,
    /// Origin X coordinate.
    x: f32,
    /// Origin Y coordinate.
    y: f32,
    /// Width in pixels.
    width: f32,
    /// Height in pixels.
    height: f32,
}

/// Enumerate active displays and produce simple bounding frames.
fn enumerate_displays() -> Result<Vec<DisplayFrame>> {
    let mut frames = Vec::new();
    if let Ok(active) = CGDisplay::active_displays() {
        for id in active {
            let display = CGDisplay::new(id);
            let bounds: CGRect = display.bounds();
            frames.push(DisplayFrame {
                id: display.id,
                x: bounds.origin.x as f32,
                y: bounds.origin.y as f32,
                width: bounds.size.width as f32,
                height: bounds.size.height as f32,
            });
        }
    }

    if frames.is_empty() {
        return Err(Error::InvalidState("no active displays detected".into()));
    }
    Ok(frames)
}

/// Resolve the display identifier containing the currently focused window.
fn focused_display_id(displays: &[DisplayFrame]) -> Option<u32> {
    let options = kCGWindowListOptionOnScreenOnly | kCGWindowListExcludeDesktopElements;
    let arr: CFArray = copy_window_info(options, kCGNullWindowID)?;
    let key_layer = unsafe { CFString::wrap_under_get_rule(kCGWindowLayer) };
    let key_bounds = unsafe { CFString::wrap_under_get_rule(kCGWindowBounds) };

    for raw in arr.iter() {
        let dict_ptr = *raw;
        let dict: CFDictionary<CFString, CFType> =
            unsafe { CFDictionary::wrap_under_get_rule(dict_ptr as _) };
        let layer = dict_value_i32(&dict, &key_layer).unwrap_or(1);
        if layer != 0 {
            continue;
        }
        let display = dict_value_rect(&dict, &key_bounds)
            .as_ref()
            .and_then(|rect| display_for_rect(rect, displays));
        if display.is_some() {
            return display;
        }
        break;
    }
    None
}

/// Pick the display that contains the majority of a rectangle.
fn display_for_rect(bounds: &CGRect, displays: &[DisplayFrame]) -> Option<u32> {
    if displays.is_empty() {
        return None;
    }

    let center_x = (bounds.origin.x + bounds.size.width * 0.5) as f32;
    let center_y = (bounds.origin.y + bounds.size.height * 0.5) as f32;

    if let Some(display) = displays
        .iter()
        .find(|d| point_in_display(d, center_x, center_y))
    {
        return Some(display.id);
    }

    displays
        .iter()
        .map(|d| (d.id, overlap_area(bounds, d)))
        .filter(|(_, area)| *area > 0.0)
        .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(Ordering::Equal))
        .map(|(id, _)| id)
}

/// Check whether a point lies within a display frame.
fn point_in_display(display: &DisplayFrame, x: f32, y: f32) -> bool {
    x >= display.x
        && x <= display.x + display.width
        && y >= display.y
        && y <= display.y + display.height
}

/// Compute the area of overlap between a rect and a display.
fn overlap_area(bounds: &CGRect, display: &DisplayFrame) -> f32 {
    let left = bounds.origin.x.max(display.x as f64) as f32;
    let right =
        (bounds.origin.x + bounds.size.width).min((display.x + display.width) as f64) as f32;
    let bottom = bounds.origin.y.max(display.y as f64) as f32;
    let top =
        (bounds.origin.y + bounds.size.height).min((display.y + display.height) as f64) as f32;

    let width = (right - left).max(0.0);
    let height = (top - bottom).max(0.0);
    width * height
}

/// Read a string from a CoreGraphics window dictionary value.
fn dict_value_string(dict: &CFDictionary<CFString, CFType>, key: &CFString) -> Option<String> {
    dict.find(key)
        .and_then(|v: ItemRef<CFType>| v.downcast::<CFString>())
        .map(|s: CFString| s.to_string())
}

/// Read a boolean from a CoreGraphics window dictionary value.
fn dict_value_bool(dict: &CFDictionary<CFString, CFType>, key: &CFString) -> Option<bool> {
    dict.find(key)
        .and_then(|v: ItemRef<CFType>| v.downcast::<CFNumber>())
        .and_then(|n: CFNumber| n.to_i64())
        .map(|n| n != 0)
}

/// Read an i32 from a CoreGraphics window dictionary value.
fn dict_value_i32(dict: &CFDictionary<CFString, CFType>, key: &CFString) -> Option<i32> {
    dict.find(key)
        .and_then(|v: ItemRef<CFType>| v.downcast::<CFNumber>())
        .and_then(|n: CFNumber| n.to_i64())
        .map(|n| n as i32)
}

/// Read a u32 from a CoreGraphics window dictionary value.
fn dict_value_u32(dict: &CFDictionary<CFString, CFType>, key: &CFString) -> Option<u32> {
    dict_value_i32(dict, key).map(|v| v as u32)
}

/// Extract a CGRect from a CoreGraphics window dictionary.
fn dict_value_rect(dict: &CFDictionary<CFString, CFType>, key: &CFString) -> Option<CGRect> {
    let bounds_dict: CFDictionary<CFString, CFType> =
        unsafe { CFDictionary::wrap_under_get_rule(dict.find(key)?.as_CFTypeRef() as _) };
    let x = dict_value_f32(&bounds_dict, "X")?;
    let y = dict_value_f32(&bounds_dict, "Y")?;
    let width = dict_value_f32(&bounds_dict, "Width")?;
    let height = dict_value_f32(&bounds_dict, "Height")?;
    let origin = CGPoint::new(x as f64, y as f64);
    let size = CGSize::new(width as f64, height as f64);
    Some(CGRect::new(&origin, &size))
}

/// Read an f32 from a CoreGraphics window dictionary entry.
fn dict_value_f32(dict: &CFDictionary<CFString, CFType>, name: &'static str) -> Option<f32> {
    let key = CFString::from_static_string(name);
    dict.find(&key)
        .and_then(|v: ItemRef<CFType>| v.downcast::<CFNumber>())
        .and_then(|n: CFNumber| n.to_f64())
        .map(|v| v as f32)
}
