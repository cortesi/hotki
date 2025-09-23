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
    pub fn activate_until_ready(&mut self, timeout_ms: u64) -> Result<()> {
        let expected = vec![canonicalize(ACTIVATION_IDENT)];
        ui_interaction::send_activation_chord()?;
        self.wait_for_hotkeys(&expected, timeout_ms, true)
    }

    /// Wait for a nested key sequence, resending activation while ensuring the HUD stays frontmost.
    pub fn await_nested(&mut self, sequence: &[&str], timeout_ms: u64) -> Result<()> {
        if sequence.is_empty() {
            return Ok(());
        }
        let expected: Vec<String> = sequence.iter().map(|ident| canonicalize(ident)).collect();
        self.wait_for_hotkeys(&expected, timeout_ms, true)
    }

    /// Consume server events until the expected hotkeys fire in order.
    fn wait_for_hotkeys(
        &mut self,
        expected: &[String],
        timeout_ms: u64,
        keep_activation: bool,
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
                self.maybe_resend_activation(&mut last_activation)?;
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
                            if keep_activation {
                                self.assert_hud_frontmost()?;
                            }
                        } else {
                            debug!(ident = %canonical, "binding_watcher_skip");
                        }
                    }
                    MsgToUI::HudUpdate { cursor } => {
                        hud_visible |= cursor.viewing_root || cursor.depth() > 0;
                    }
                    _ => {}
                }
            }
        }

        if keep_activation {
            self.ensure_frontmost_after_sequence(hud_visible, deadline)?;
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
    fn assert_hud_frontmost(&self) -> Result<()> {
        if self.hud_frontmost()? {
            Ok(())
        } else {
            Err(Error::InvalidState(
                "HUD failed to stay frontmost while waiting for bindings".into(),
            ))
        }
    }

    /// Optionally resend the activation chord if the HUD drifts or the interval passes.
    fn maybe_resend_activation(&self, last_activation: &mut Instant) -> Result<()> {
        let need_frontmost = !self.hud_frontmost()?;
        if need_frontmost || last_activation.elapsed() >= self.resend_interval {
            ui_interaction::send_activation_chord()?;
            *last_activation = Instant::now();
        }
        Ok(())
    }

    /// After completing a sequence, guarantee the HUD is frontmost before returning.
    fn ensure_frontmost_after_sequence(&self, hud_visible: bool, deadline: Instant) -> Result<()> {
        if hud_visible && self.hud_frontmost()? {
            return Ok(());
        }
        ui_interaction::send_activation_chord()?;
        let mut attempt_deadline =
            Instant::now() + Duration::from_millis(config::INPUT_DELAYS.poll_interval_ms * 4);
        if attempt_deadline > deadline {
            attempt_deadline = deadline;
        }
        while Instant::now() < attempt_deadline {
            if self.hud_frontmost()? {
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
