//! Focus guards that reconcile world and Accessibility viewpoints during smoketests.

use std::{thread, time::Duration};

use hotki_world_ids::WorldWindowId;
use mac_winops::focus;
use tracing::debug;

use crate::{
    config,
    error::{Error, Result},
    server_drive::{self, DriverError},
    world,
};

/// Maximum number of raise attempts performed before giving up.
const MAX_ATTEMPTS: usize = 5;

/// Guard that ensures a helper window remains frontmost across world and AX views.
#[derive(Clone)]
pub struct FocusGuard {
    /// Process identifier that should remain focused.
    pid: i32,
    /// Expected window title used when reconciling focus state.
    title: String,
    /// Optional world identifier when already known.
    window: Option<WorldWindowId>,
    /// Deadline applied to smart-raise attempts.
    raise_deadline: Duration,
}

impl FocusGuard {
    /// Acquire focus for the provided `(pid, title)` pair, optionally seeding with a known window id.
    pub fn acquire(
        pid: i32,
        title: impl Into<String>,
        window: Option<WorldWindowId>,
    ) -> Result<Self> {
        let guard = Self {
            pid,
            title: title.into(),
            window,
            raise_deadline: Duration::from_millis(
                config::INPUT_DELAYS
                    .ui_action_delay_ms
                    .saturating_mul(20)
                    .max(1_000),
            ),
        };
        guard.reassert()?;
        Ok(guard)
    }

    /// Re-run the focus loop until world and AX agree on the frontmost window.
    pub fn reassert(&self) -> Result<()> {
        let mut attempts = 0usize;
        let mut last_error: Option<Error> = None;
        while attempts < MAX_ATTEMPTS {
            attempts += 1;
            if let Err(err) = self.raise_once() {
                debug!(pid = self.pid, title = %self.title, attempt = attempts, error = %err, "focus_guard_raise_failed");
                last_error = Some(err);
            }
            match self.verify_alignment() {
                Ok(true) => return Ok(()),
                Ok(false) => {}
                Err(err) => {
                    debug!(pid = self.pid, title = %self.title, attempt = attempts, error = %err, "focus_guard_verify_failed");
                    last_error = Some(err);
                }
            }
            thread::sleep(Duration::from_millis(config::INPUT_DELAYS.retry_delay_ms));
        }
        Err(last_error.unwrap_or_else(|| {
            Error::InvalidState(format!(
                "focus guard: window '{}' (pid {}) failed to settle",
                self.title, self.pid
            ))
        }))
    }

    /// Attempt a single raise pass using world and smart-raise helpers.
    fn raise_once(&self) -> Result<()> {
        let mut last_error: Option<Error> = None;
        if let Err(err) = world::ensure_frontmost(
            self.pid,
            &self.title,
            4,
            config::INPUT_DELAYS.ui_action_delay_ms,
        ) {
            last_error = Some(err);
        }

        if let Some(id) = self.resolve_window_id()?
            && let Err(err) = world::smart_raise(id, &self.title, self.raise_deadline)
        {
            last_error = Some(err);
        }

        if let Some(err) = last_error {
            Err(err)
        } else {
            Ok(())
        }
    }

    /// Resolve the world window identifier if not already known.
    fn resolve_window_id(&self) -> Result<Option<WorldWindowId>> {
        if let Some(id) = self.window {
            return Ok(Some(id));
        }
        let windows = world::list_windows()?;
        Ok(windows
            .iter()
            .find(|w| w.pid == self.pid && w.title == self.title)
            .map(|w| WorldWindowId::new(self.pid, w.id)))
    }

    /// Confirm that world and AX agree on the focused window.
    fn verify_alignment(&self) -> Result<bool> {
        let world_ok = match server_drive::get_world_snapshot() {
            Ok(snapshot) => snapshot
                .windows
                .iter()
                .find(|w| w.pid == self.pid && w.title == self.title)
                .map(|w| w.focused && w.on_active_space)
                .unwrap_or(false),
            Err(DriverError::NotInitialized) => {
                debug!(pid = self.pid, title = %self.title, "focus_guard_skip_world");
                true
            }
            Err(err) => return Err(Error::from(err)),
        };

        if !world_ok {
            debug!(pid = self.pid, title = %self.title, "focus_guard_world_pending");
            return Ok(false);
        }

        let ax_ok = match focus::system_focus_snapshot() {
            Some((_, title, pid)) => pid == self.pid && (title.is_empty() || title == self.title),
            None => false,
        };

        if !ax_ok {
            debug!(pid = self.pid, title = %self.title, "focus_guard_ax_pending");
        }

        Ok(world_ok && ax_ok)
    }
}
