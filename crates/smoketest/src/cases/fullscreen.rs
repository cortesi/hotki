//! Fullscreen smoketest case implemented on the registry runner.

use std::{
    fs, thread,
    time::{Duration, Instant},
};

use mac_winops;
use tracing::warn;

use super::support::{
    ScenarioState, WindowSpawnSpec, raise_window, shutdown_mimic, spawn_scenario,
};
use crate::{
    config,
    error::{Error, Result},
    server_drive,
    session::{HotkiSession, HotkiSessionConfig},
    suite::{CaseCtx, sanitize_slug},
    world::{self, FocusGuard},
};

/// Scenario slug and label used for the fullscreen helper window.
const HELPER_SCENARIO_SLUG: &str = "fullscreen.helper";
/// Label assigned to the fullscreen helper inside the scenario.
const HELPER_LABEL: &str = "primary";
/// Overlay text rendered inside the fullscreen helper window.
const HELPER_LABEL_TEXT: &str = "FS";

/// Toggle non-native fullscreen for a helper window and capture before/after diagnostics.
pub fn fullscreen_toggle_nonnative(ctx: &mut CaseCtx<'_>) -> Result<()> {
    let mut state: Option<FullscreenCaseState> = None;

    ctx.setup(|ctx| {
        let helper_title_base = config::test_title("fullscreen");
        let helper_title = format!(
            "{} [{}::{}]",
            helper_title_base, HELPER_SCENARIO_SLUG, HELPER_LABEL
        );
        let config_ron = build_fullscreen_config(&helper_title);
        let filename = format!("{}_config.ron", sanitize_slug(ctx.case_name()));
        let config_path = ctx.scratch_path(filename);
        fs::write(&config_path, config_ron.as_bytes())?;

        let session = HotkiSession::spawn(
            HotkiSessionConfig::from_env()?
                .with_config(&config_path)
                .with_logs(true),
        )?;

        let helper_lifetime = config::HELPER_WINDOW
            .default_lifetime_ms
            .saturating_add(config::HELPER_WINDOW.extra_time_ms);
        let helper_spec =
            WindowSpawnSpec::new(HELPER_LABEL, helper_title_base).configure(move |config| {
                config.time_ms = helper_lifetime;
                config.label_text = Some(HELPER_LABEL_TEXT.into());
            });
        let scenario = spawn_scenario(ctx, HELPER_SCENARIO_SLUG, vec![helper_spec])?;

        state = Some(FullscreenCaseState {
            session,
            title: helper_title,
            scenario,
            before: None,
            after: None,
        });
        Ok(())
    })?;

    ctx.action(|ctx| {
        let state_ref = state
            .as_mut()
            .ok_or_else(|| Error::InvalidState("fullscreen state missing during action".into()))?;

        let socket = state_ref.session.socket_path().to_string();
        server_drive::ensure_init(&socket, 5_000)?;
        server_drive::wait_for_idents(&["g", "shift+cmd+9"], 4_000)?;

        let helper_pid;
        let helper_world_id;
        {
            let window = state_ref.scenario.window(HELPER_LABEL)?;
            helper_pid = window.key.pid;
            helper_world_id = window.world_id;
        }

        raise_window(ctx, &mut state_ref.scenario, HELPER_LABEL)?;
        if let Err(err) = world::ensure_frontmost(
            helper_pid,
            &state_ref.title,
            3,
            config::INPUT_DELAYS.ui_action_delay_ms,
        ) {
            warn!(
                "ensure_frontmost: world mediation failed ({}); falling back to AX raise",
                err
            );
            if !mac_winops::ensure_frontmost_by_title(
                helper_pid,
                &state_ref.title,
                3,
                config::INPUT_DELAYS.ui_action_delay_ms,
            ) {
                warn!(
                    "ensure_frontmost: mac_winops fallback failed pid={} title='{}'",
                    helper_pid, state_ref.title
                );
            }
        }

        let focus_guard = FocusGuard::acquire(helper_pid, &state_ref.title, Some(helper_world_id))?;

        server_drive::inject_key("g")?;
        focus_guard.reassert()?;
        let before = read_ax_frame(helper_pid, &state_ref.title)?;

        server_drive::inject_key("shift+cmd+9")?;
        let after = wait_for_frame_update(
            helper_pid,
            &state_ref.title,
            config::FULLSCREEN.wait_total_ms,
        )?;
        focus_guard.reassert()?;

        state_ref.before = Some(before);
        state_ref.after = Some(after);

        ctx.log_event("fullscreen_runtime", "fullscreen toggle executed");

        Ok(())
    })?;

    ctx.settle(|ctx| {
        let mut state_inner = state
            .take()
            .ok_or_else(|| Error::InvalidState("fullscreen state missing during settle".into()))?;

        let before = state_inner
            .before
            .ok_or_else(|| Error::InvalidState("missing before frame".into()))?;
        let after = state_inner
            .after
            .ok_or_else(|| Error::InvalidState("missing after frame".into()))?;
        let area_delta = after.area() - before.area();

        ctx.log_event(
            "fullscreen_outcome",
            &format!(
                "helper_title={} before=({:.1},{:.1},{:.1},{:.1}) after=({:.1},{:.1},{:.1},{:.1}) area_delta={}",
                state_inner.title,
                before.x,
                before.y,
                before.w,
                before.h,
                after.x,
                after.y,
                after.w,
                after.h,
                area_delta
            ),
        );

        shutdown_mimic(state_inner.scenario.mimic)?;
        state_inner.session.shutdown();
        state_inner.session.kill_and_wait();
        server_drive::reset();
        Ok(())
    })?;

    Ok(())
}

/// Case state retained across stages for the fullscreen scenario.
struct FullscreenCaseState {
    /// Running Hotki session used to drive the toggle.
    session: HotkiSession,
    /// Title assigned to the helper window.
    title: String,
    /// Active mimic scenario managing the fullscreen helper window.
    scenario: ScenarioState,
    /// Frame captured before toggling fullscreen.
    before: Option<AxFrame>,
    /// Frame captured after toggling fullscreen.
    after: Option<AxFrame>,
}

/// Compose a fullscreen configuration embedding the helper title.
fn build_fullscreen_config(title: &str) -> String {
    format!(
        "(\n        keys: [\n            (\"g\", \"raise\", raise(title: \"{}\"), (noexit: true)),\n            (\"shift+cmd+9\", \"Fullscreen\", fullscreen(toggle) , (global: true)),\n        ],\n        style: (hud: (mode: hide)),\n        server: (exit_if_no_clients: true),\n    )",
        title
    )
}

/// Read the current AX frame for `(pid, title)`.
fn read_ax_frame(pid: i32, title: &str) -> Result<AxFrame> {
    mac_winops::ax_window_frame(pid, title)
        .map(AxFrame::from_tuple)
        .ok_or_else(|| Error::InvalidState("failed to read initial window frame".into()))
}

/// Wait until the helper window reports an updated AX frame.
fn wait_for_frame_update(pid: i32, title: &str, timeout_ms: u64) -> Result<AxFrame> {
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    loop {
        if let Some(frame) = mac_winops::ax_window_frame(pid, title) {
            return Ok(AxFrame::from_tuple(frame));
        }
        if Instant::now() >= deadline {
            return Err(Error::InvalidState(
                "failed to read window frame after fullscreen toggle".into(),
            ));
        }
        if server_drive::check_alive().is_err() {
            return Err(Error::IpcDisconnected {
                during: "fullscreen toggle",
            });
        }
        thread::sleep(Duration::from_millis(config::FULLSCREEN.wait_poll_ms));
    }
}

/// Simplified representation of a helper window frame captured via AX.
#[derive(Clone, Debug)]
struct AxFrame {
    /// Window origin X coordinate.
    x: f64,
    /// Window origin Y coordinate.
    y: f64,
    /// Window width in pixels.
    w: f64,
    /// Window height in pixels.
    h: f64,
}

impl AxFrame {
    /// Construct an [`AxFrame`] from the tuple returned by `ax_window_frame`.
    fn from_tuple(raw: ((f64, f64), (f64, f64))) -> Self {
        let ((x, y), (w, h)) = raw;
        Self { x, y, w, h }
    }

    /// Compute the frame area in pixels.
    fn area(&self) -> f64 {
        self.w * self.h
    }
}
