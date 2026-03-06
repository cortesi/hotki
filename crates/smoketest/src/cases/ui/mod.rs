//! UI-driven smoketest cases executed via the registry runner.

/// Activation polling and HUD-ready detection helpers.
mod activation;
/// CoreGraphics window and display inspection helpers.
mod window_inspection;

use std::{fs, path::PathBuf, thread, time::Duration};

use tracing::{debug, info};

use self::{
    activation::{ActivationOutcome, BindingWatcher},
    window_inspection::{HudWindowSnapshot, collect_hud_windows},
};
use crate::{
    config,
    error::{Error, Result},
    session::{HotkiSession, HotkiSessionConfig},
    suite::{CaseCtx, LOG_TARGET, sanitize_slug},
};

/// Canonical activation chord identifier expected from the server.
pub const ACTIVATION_IDENT: &str = "shift+cmd+0";

/// Key sequence applied once the HUD is visible to exercise the demo flow.
const UI_DEMO_SEQUENCE: &[&str] = &["t", "l", "l", "l", "l", "l", "n", "esc"];

/// Demo HUD mode applied in the generated Rhai config.
#[derive(Clone, Copy)]
enum DemoHudMode {
    /// Full HUD mode anchored bottom-right.
    Hud,
    /// Compact mini HUD mode.
    Mini,
}

impl DemoHudMode {
    /// Render the Rhai token used for the HUD mode.
    fn rhai_value(self) -> &'static str {
        match self {
            Self::Hud => "hud",
            Self::Mini => "mini",
        }
    }
}

/// Build the standard demo config used by the UI smoketests.
fn demo_config(mode: DemoHudMode) -> String {
    format!(
        r#"
theme("default");

hotki.mode(|m, ctx| {{
  m.style(#{{
    hud: #{{
      mode: {},
      pos: se,
    }},
  }});

  m.mode("shift+cmd+0", "activate", |m, ctx| {{
    m.mode("t", "Theme tester", |sub, ctx| {{
      sub.bind("h", "Theme Prev", action.theme_prev).stay();
      sub.bind("l", "Theme Next", action.theme_next).stay();
      sub.bind("n", "Notify", action.shell("echo notify").notify(info, warn)).stay();
      sub.bind("esc", "Exit", action.exit).hidden();
    }});
  }});
}});
"#,
        mode.rhai_value()
    )
}

/// Verify full HUD appears and responds to keys.
pub fn hud(ctx: &mut CaseCtx<'_>) -> Result<()> {
    run_ui_case(
        ctx,
        &UiCaseSpec {
            slug: "hud",
            hud_mode: DemoHudMode::Hud,
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
            hud_mode: DemoHudMode::Mini,
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
            hud_mode: DemoHudMode::Hud,
            with_logs: true,
        },
        verify_display_alignment,
    )
}

/// Parameters describing a UI demo scenario.
struct UiCaseSpec {
    /// Registry slug recorded in artifacts.
    slug: &'static str,
    /// HUD mode injected into the generated Rhai configuration.
    hud_mode: DemoHudMode,
    /// Whether to enable verbose logging for the child process.
    with_logs: bool,
}

impl UiCaseSpec {
    /// Render the Rhai config used by this scenario.
    fn render_config(&self) -> String {
        demo_config(self.hud_mode)
    }
}

/// Mutable state shared across UI case stages.
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
        let rhai_config = spec.render_config();
        fs::write(&config_path, rhai_config.as_bytes())?;

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
        if !activation.focus_event_seen() {
            info!(
                target: LOG_TARGET,
                pid = state_ref.session.pid(),
                "no focus-change bridge events observed during activation"
            );
        }

        {
            let bridge = state_ref.session.bridge_mut();
            bridge.inject_key(ident_activate)?;
            bridge.wait_for_idents(&["h", "l", "n"], gate_ms)?;
        }

        for key in &UI_DEMO_SEQUENCE[1..UI_DEMO_SEQUENCE.len() - 1] {
            let bridge = state_ref.session.bridge_mut();
            bridge.inject_key(key)?;
            thread::sleep(Duration::from_millis(config::THEME_SWITCH_DELAY_MS));
        }

        let hud_windows = collect_hud_windows(state_ref.session.pid() as i32)?;

        state_ref.time_to_hud_ms = activation.hud_visible_ms();
        state_ref.hud_activation = Some(activation);
        state_ref.hud_windows = Some(hud_windows);

        after_activation(state_ref)?;

        let ident_exit = UI_DEMO_SEQUENCE[UI_DEMO_SEQUENCE.len() - 1];
        state_ref.session.bridge_mut().inject_key(ident_exit)?;
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

        if let Err(err) = state_inner.session.shutdown() {
            debug!(
                ?err,
                "bridge shutdown returned error (expected on clean exit)"
            );
        }
        state_inner.session.kill_and_wait();

        Ok(())
    })?;

    Ok(())
}

/// Verify the HUD aligns with the active display reported by the world service.
fn verify_display_alignment(state: &mut UiCaseState) -> Result<()> {
    let displays = window_inspection::enumerate_displays()?;
    if displays.len() < 2 {
        info!(
            target: LOG_TARGET,
            pid = state.session.pid(),
            count = displays.len(),
            "skipping display mapping check: fewer than two displays",
        );
        return Ok(());
    }

    let focused_display = window_inspection::focused_display_id(&displays).ok_or_else(|| {
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
        .map(|display| display.id)
        .ok_or_else(|| Error::InvalidState("HUD snapshot missing active display".into()))?;

    if hud_active != focused_display {
        return Err(Error::InvalidState(format!(
            "active display mismatch: hud={} focused={}",
            hud_active, focused_display
        )));
    }

    let hud_windows = if let Some(windows) = state.hud_windows.clone() {
        windows
    } else {
        let windows = collect_hud_windows(state.session.pid() as i32)?;
        state.hud_windows = Some(windows.clone());
        windows
    };

    let mut mapped: Vec<u32> = hud_windows
        .iter()
        .filter_map(|window| window.display_id)
        .collect();
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
