use std::{
    thread,
    time::{Duration, Instant},
};

use tracing::debug;

use super::ACTIVATION_IDENT;
use crate::{
    config,
    error::{Error, Result},
    server_drive::{BridgeClient, BridgeEvent, DriverError},
};

/// Result payload describing an activation attempt.
#[derive(Debug, Clone)]
pub(super) struct ActivationOutcome {
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
    pub(super) fn hud_visible_ms(&self) -> Option<u64> {
        self.hud_update_ms.or(self.frontmost_ms)
    }

    /// Whether a focus-change event was observed while activating.
    pub(super) fn focus_event_seen(&self) -> bool {
        self.focus_event_seen
    }

    /// Whether the HUD visibility was confirmed via polling observations.
    fn observed_via_event(&self) -> bool {
        self.hud_update_ms.is_some()
    }

    /// Render a concise summary string for logging.
    pub(super) fn summary_string(&self) -> String {
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
pub(super) struct BindingWatcher;

impl BindingWatcher {
    /// Create a watcher scoped to the target HUD process id.
    pub(super) fn new(_hud_pid: i32) -> Self {
        Self
    }

    /// Attempt to activate the HUD and wait until the expected submenu bindings appear.
    pub(super) fn activate_until_ready(
        &self,
        bridge: &mut BridgeClient,
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
                            BridgeEvent::Hud { ref hud, .. } => {
                                let parent_title = hud.breadcrumbs.last();
                                debug!(
                                    event_id = event.id,
                                    event_ms = event.timestamp_ms,
                                    depth = hud.depth,
                                    parent = ?parent_title,
                                    visible = hud.visible,
                                    row_count = hud.rows.len(),
                                    "hud_event_observed"
                                );
                                if last_hud_event == Some(event.id) {
                                    continue;
                                }
                                metrics.record_hud_update();
                                metrics.record_frontmost();
                                last_hud_event = Some(event.id);
                                if let Ok(Some(snapshot)) = bridge.latest_hud() {
                                    debug!(
                                        hud_event_id = snapshot.event_id,
                                        depth = snapshot.hud.depth,
                                        parent = ?snapshot.hud.breadcrumbs.last(),
                                        received_ms = snapshot.received_ms,
                                        visible = snapshot.hud.visible,
                                        row_count = snapshot.hud.rows.len(),
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
                    ACTIVATION_IDENT,
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
        bridge: &mut BridgeClient,
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
    fn hud_visible(&self, bridge: &BridgeClient) -> Result<bool> {
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
        .and_then(|duration| {
            let ms = duration.as_millis() as u64;
            if ms > 0 { Some(ms) } else { None }
        })
}
