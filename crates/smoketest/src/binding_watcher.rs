//! HUD readiness utilities used by UI smoketests.

use std::{
    thread,
    time::{Duration, Instant},
};

use crate::{
    config,
    error::{Error, Result},
    server_drive::{self, DriverError},
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
    pub fn hud_visible_ms(&self) -> Option<u64> {
        self.hud_update_ms.or(self.frontmost_ms)
    }

    /// Whether the HUD visibility was confirmed via polling observations.
    pub fn observed_via_event(&self) -> bool {
        self.hud_update_ms.is_some()
    }

    /// Render a concise summary string for logging.
    pub fn summary_string(&self) -> String {
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

/// Observes HUD visibility while waiting for activation to settle.
pub struct BindingWatcher {
    /// PID of the HUD window used to verify visibility.
    hud_pid: i32,
}

impl BindingWatcher {
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

    /// Create a watcher scoped to the target HUD process id.
    pub fn connect(_socket_path: &str, hud_pid: i32) -> Result<Self> {
        Ok(Self { hud_pid })
    }

    /// Attempt to activate the HUD and wait until the expected submenu bindings appear.
    pub fn activate_until_ready(
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

        self.dispatch_activation(
            activation_ident,
            deadline,
            timeout_ms,
            &mut metrics,
            &mut last_attempt,
        )?;

        while Instant::now() < deadline {
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

    /// Check whether the HUD window is visible on the active space.
    fn hud_visible(&self) -> Result<bool> {
        let windows = world::list_windows()?;
        Ok(windows
            .iter()
            .any(|w| w.pid == self.hud_pid && w.title == HUD_TITLE && w.is_on_screen))
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
