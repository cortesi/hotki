//! Event-driven binding watcher utilities used by UI smoketests.

use std::{
    cmp, thread,
    time::{Duration, Instant},
};

use hotki_protocol::MsgToUI;
use mac_keycode::Chord;
use tokio::time::timeout;
use tracing::debug;

use crate::{
    config,
    error::{Error, Result},
    runtime, ui_interaction, world,
};

/// Window title emitted by the HUD process.
const HUD_TITLE: &str = "Hotki HUD";
/// Canonical activation chord identifier expected from the server.
const ACTIVATION_IDENT: &str = "shift+cmd+0";

/// Result payload describing an activation attempt.
#[derive(Debug, Clone)]
pub struct ActivationOutcome {
    /// Time (ms) from activation send until a HUD update event was observed.
    hud_update_ms: Option<u64>,
    /// Time (ms) from activation send until the HUD was confirmed frontmost.
    frontmost_ms: Option<u64>,
    /// Number of activation chord injections issued while waiting.
    activation_attempts: u32,
    /// How many HUD update events were seen in total.
    hud_updates_seen: u32,
}

impl ActivationOutcome {
    /// Earliest timing when the HUD was confirmed visible.
    pub fn hud_visible_ms(&self) -> Option<u64> {
        self.hud_update_ms.or(self.frontmost_ms)
    }

    /// Whether the HUD visibility was confirmed via HudUpdate event.
    pub fn observed_via_event(&self) -> bool {
        self.hud_update_ms.is_some()
    }

    /// Marshal the outcome into a JSON-friendly representation.
    pub fn to_summary_json(&self) -> serde_json::Value {
        serde_json::json!({
            "hud_update_ms": self.hud_update_ms,
            "frontmost_ms": self.frontmost_ms,
            "activation_attempts": self.activation_attempts,
            "hud_updates_seen": self.hud_updates_seen,
            "observed_via_event": self.observed_via_event(),
        })
    }
}

/// Internal activation timing accumulator.
#[derive(Debug)]
struct ActivationMetrics {
    /// Activation start instant used as the timing baseline.
    start: Instant,
    /// Instant captured when the first HudUpdate event reported visibility.
    hud_update_at: Option<Instant>,
    /// Instant captured when the HUD was first confirmed frontmost.
    frontmost_at: Option<Instant>,
    /// Number of activation chords injected while waiting.
    activation_attempts: u32,
    /// Count of HudUpdate events observed during activation.
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

    /// Record a HudUpdate event observed while waiting for visibility.
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

/// Thin helper that gracefully handles optional metrics tracking.
struct MetricsCtx<'a> {
    /// Optional metrics accumulator shared with helper routines.
    metrics: Option<&'a mut ActivationMetrics>,
}

impl<'a> MetricsCtx<'a> {
    /// Wrap a live metrics accumulator.
    fn some(metrics: &'a mut ActivationMetrics) -> Self {
        Self {
            metrics: Some(metrics),
        }
    }

    /// Construct a metrics context that performs no-op recording.
    fn none() -> Self {
        Self { metrics: None }
    }

    /// Record an activation send if metrics tracking is active.
    fn record_activation(&mut self) {
        if let Some(metrics) = self.metrics.as_mut() {
            (**metrics).record_activation();
        }
    }

    /// Record a HudUpdate event if metrics tracking is active.
    fn record_hud_update(&mut self) {
        if let Some(metrics) = self.metrics.as_mut() {
            (**metrics).record_hud_update();
        }
    }

    /// Record a frontmost confirmation if metrics tracking is active.
    fn record_frontmost(&mut self) {
        if let Some(metrics) = self.metrics.as_mut() {
            (**metrics).record_frontmost();
        }
    }
}

/// Observes hotki-server events and keeps the HUD responsive while bindings settle.
pub struct BindingWatcher {
    /// Connected client instance used to receive server events.
    client: hotki_server::Client,
    /// PID of the HUD window used to verify frontmost visibility.
    hud_pid: i32,
    /// Interval between activation resend attempts.
    resend_interval: Duration,
}

impl BindingWatcher {
    /// Connect a watcher to the running server using its socket path and HUD pid.
    pub fn connect(socket_path: &str, hud_pid: i32) -> Result<Self> {
        let client = match runtime::block_on(async {
            hotki_server::Client::new_with_socket(socket_path)
                .with_connect_only()
                .connect()
                .await
        }) {
            Ok(Ok(client)) => client,
            Ok(Err(err)) => {
                return Err(Error::InvalidState(format!(
                    "binding watcher failed to connect: {err}"
                )));
            }
            Err(err) => return Err(err),
        };

        Ok(Self {
            client,
            hud_pid,
            resend_interval: Duration::from_millis(config::SESSION.activation_resend_interval_ms),
        })
    }

    /// Drive the activation chord until it registers and the HUD is frontmost.
    pub fn activate_until_ready(&mut self, timeout_ms: u64) -> Result<ActivationOutcome> {
        let expected = vec![canonicalize(ACTIVATION_IDENT)];
        let start = Instant::now();
        ui_interaction::send_activation_chord()?;
        let mut metrics = ActivationMetrics::new(start);
        metrics.record_activation();
        {
            let mut ctx = MetricsCtx::some(&mut metrics);
            self.wait_for_hotkeys(&expected, timeout_ms, true, &mut ctx)?;
        }
        Ok(metrics.into_outcome())
    }

    /// Wait for a nested key sequence, resending activation while ensuring the HUD stays frontmost.
    pub fn await_nested(&mut self, sequence: &[&str], timeout_ms: u64) -> Result<()> {
        if sequence.is_empty() {
            return Ok(());
        }
        let expected: Vec<String> = sequence.iter().map(|ident| canonicalize(ident)).collect();
        let mut ctx = MetricsCtx::none();
        self.wait_for_hotkeys(&expected, timeout_ms, true, &mut ctx)
    }

    /// Consume server events until the expected hotkeys fire in order.
    fn wait_for_hotkeys(
        &mut self,
        expected: &[String],
        timeout_ms: u64,
        keep_activation: bool,
        metrics: &mut MetricsCtx<'_>,
    ) -> Result<()> {
        let mut remaining = expected;
        let mut last_activation = Instant::now();
        if keep_activation {
            last_activation -= self.resend_interval;
        }
        let mut hud_visible = false;
        let deadline = Instant::now() + Duration::from_millis(timeout_ms);

        while !remaining.is_empty() {
            if Instant::now() >= deadline {
                return Err(Error::HudNotVisible { timeout_ms });
            }

            if keep_activation {
                self.maybe_resend_activation(&mut last_activation, metrics)?;
            }

            let wait = cmp::min(
                Duration::from_millis(config::INPUT_DELAYS.poll_interval_ms),
                deadline.saturating_duration_since(Instant::now()),
            );
            let msg = self.poll_event(wait)?;
            if let Some(msg) = msg {
                match msg {
                    MsgToUI::HotkeyTriggered(raw) => {
                        let canonical = canonicalize(raw.as_str());
                        if canonical == remaining[0] {
                            remaining = &remaining[1..];
                            debug!(ident = %canonical, "binding_watcher_match");
                            if keep_activation && self.assert_hud_frontmost()? {
                                metrics.record_frontmost();
                                hud_visible = true;
                            }
                        } else {
                            debug!(ident = %canonical, "binding_watcher_skip");
                        }
                    }
                    MsgToUI::HudUpdate { cursor } => {
                        hud_visible |= cursor.viewing_root || cursor.depth() > 0;
                        if hud_visible {
                            metrics.record_hud_update();
                        }
                    }
                    _ => {}
                }
            }
        }

        if keep_activation {
            self.ensure_frontmost_after_sequence(hud_visible, deadline, metrics)?;
        }
        Ok(())
    }

    /// Block for the next server event up to `wait`, surfacing disconnects distinctly.
    fn poll_event(&mut self, wait: Duration) -> Result<Option<MsgToUI>> {
        let conn = self.client.connection().map_err(|err| {
            Error::InvalidState(format!("binding watcher lost connection: {err}"))
        })?;
        match runtime::block_on(async { timeout(wait, conn.recv_event()).await }) {
            Ok(Ok(Ok(msg))) => Ok(Some(msg)),
            Ok(Ok(Err(_))) => Err(Error::IpcDisconnected {
                during: "binding watcher event pump",
            }),
            Ok(Err(_)) => Ok(None),
            Err(err) => Err(err),
        }
    }

    /// Ensure the HUD remains the active foreground window.
    fn assert_hud_frontmost(&self) -> Result<bool> {
        if self.hud_frontmost()? {
            Ok(true)
        } else {
            Err(Error::InvalidState(
                "HUD failed to stay frontmost while waiting for bindings".into(),
            ))
        }
    }

    /// Optionally resend the activation chord if the HUD drifts or the interval passes.
    fn maybe_resend_activation(
        &self,
        last_activation: &mut Instant,
        metrics: &mut MetricsCtx<'_>,
    ) -> Result<()> {
        let need_frontmost = !self.hud_frontmost()?;
        if need_frontmost || last_activation.elapsed() >= self.resend_interval {
            ui_interaction::send_activation_chord()?;
            metrics.record_activation();
            *last_activation = Instant::now();
        }
        Ok(())
    }

    /// After completing a sequence, guarantee the HUD is frontmost before returning.
    fn ensure_frontmost_after_sequence(
        &self,
        hud_visible: bool,
        deadline: Instant,
        metrics: &mut MetricsCtx<'_>,
    ) -> Result<()> {
        if hud_visible && self.hud_frontmost()? {
            metrics.record_frontmost();
            return Ok(());
        }
        ui_interaction::send_activation_chord()?;
        metrics.record_activation();
        let mut attempt_deadline =
            Instant::now() + Duration::from_millis(config::INPUT_DELAYS.poll_interval_ms * 4);
        if attempt_deadline > deadline {
            attempt_deadline = deadline;
        }
        while Instant::now() < attempt_deadline {
            if self.hud_frontmost()? {
                metrics.record_frontmost();
                return Ok(());
            }
            thread::sleep(Duration::from_millis(40));
        }
        Err(Error::InvalidState(
            "HUD failed to remain frontmost after activation".into(),
        ))
    }

    /// Check whether the HUD window is the focused window on the active space.
    fn hud_frontmost(&self) -> Result<bool> {
        let windows = world::list_windows()?;
        Ok(windows.iter().any(|w| {
            w.pid == self.hud_pid && w.title == HUD_TITLE && w.focused && w.on_active_space
        }))
    }
}

/// Canonicalize a chord identifier to align with server formatting.
fn canonicalize(raw: &str) -> String {
    Chord::parse(raw)
        .map(|c| c.to_string())
        .unwrap_or_else(|| raw.to_string())
}
