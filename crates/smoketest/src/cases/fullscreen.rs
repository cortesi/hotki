//! Fullscreen smoketest case implemented on the registry runner.

use std::{
    cmp, env, fs,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use mac_winops::{self, wait::wait_for_windows_visible_ms};
use tracing::warn;

use crate::{
    config,
    error::{Error, Result},
    focus_guard::FocusGuard,
    process::spawn_managed,
    server_drive,
    session::HotkiSession,
    suite::{CaseCtx, sanitize_slug},
    util, world,
};

/// Toggle non-native fullscreen for a helper window and capture before/after diagnostics.
pub fn fullscreen_toggle_nonnative(ctx: &mut CaseCtx<'_>) -> Result<()> {
    let mut state: Option<FullscreenCaseState> = None;

    ctx.setup(|stage| {
        let hotki_bin = util::resolve_hotki_bin().ok_or(Error::HotkiBinNotFound)?;
        let title = config::test_title("fullscreen");
        let config_ron = build_fullscreen_config(&title);
        let filename = format!("{}_config.ron", sanitize_slug(stage.case_name()));
        let config_path = stage.scratch_path(filename);
        fs::write(&config_path, config_ron.as_bytes())?;

        let session = HotkiSession::builder(hotki_bin)
            .with_config(&config_path)
            .with_logs(true)
            .spawn()?;

        state = Some(FullscreenCaseState {
            session,
            title,
            before: None,
            after: None,
        });
        Ok(())
    })?;

    ctx.action(|stage| {
        let state_ref = state
            .as_mut()
            .ok_or_else(|| Error::InvalidState("fullscreen state missing during action".into()))?;

        let socket = state_ref.session.socket_path().to_string();
        server_drive::ensure_init(&socket, 5_000)?;
        server_drive::wait_for_idents(&["g", "shift+cmd+9"], 4_000)?;

        let helper_lifetime = config::HELPER_WINDOW
            .default_lifetime_ms
            .saturating_add(config::HELPER_WINDOW.extra_time_ms);
        let visible_timeout = cmp::max(5_000, config::HIDE.first_window_max_ms * 3);
        let mut helper = {
            let exe = env::current_exe()?;
            let mut cmd = Command::new(exe);
            cmd.env("HOTKI_SKIP_BUILD", "1")
                .arg("focus-winhelper")
                .arg("--title")
                .arg(&state_ref.title)
                .arg("--time")
                .arg(helper_lifetime.to_string())
                .arg("--label-text")
                .arg("FS")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            spawn_managed(cmd)?
        };
        if !wait_for_windows_visible_ms(
            &[(helper.pid, &state_ref.title)],
            visible_timeout,
            config::FULLSCREEN.helper_show_delay_ms,
        ) {
            return Err(Error::FocusNotObserved {
                timeout_ms: visible_timeout,
                expected: format!("helper window '{}' not visible", state_ref.title),
            });
        }
        if let Err(err) = world::ensure_frontmost(
            helper.pid,
            &state_ref.title,
            3,
            config::INPUT_DELAYS.ui_action_delay_ms,
        ) {
            warn!(
                "ensure_frontmost: world mediation failed ({}); falling back to AX raise",
                err
            );
            if !mac_winops::ensure_frontmost_by_title(
                helper.pid,
                &state_ref.title,
                3,
                config::INPUT_DELAYS.ui_action_delay_ms,
            ) {
                warn!(
                    "ensure_frontmost: mac_winops fallback failed pid={} title='{}'",
                    helper.pid, state_ref.title
                );
            }
        }

        let focus_guard = FocusGuard::acquire(helper.pid, &state_ref.title, None)?;

        server_drive::inject_key("g")?;
        focus_guard.reassert()?;
        let before = read_ax_frame(helper.pid, &state_ref.title)?;

        server_drive::inject_key("shift+cmd+9")?;
        let after = wait_for_frame_update(
            helper.pid,
            &state_ref.title,
            config::FULLSCREEN.wait_total_ms,
        )?;
        focus_guard.reassert()?;

        helper.kill_and_wait()?;
        state_ref.before = Some(before);
        state_ref.after = Some(after);

        stage.log_event("fullscreen_runtime", "fullscreen toggle executed");

        Ok(())
    })?;

    ctx.settle(|stage| {
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

        stage.log_event(
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
