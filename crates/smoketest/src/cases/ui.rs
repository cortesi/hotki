//! UI-driven smoketest cases executed via the registry runner.

use std::{fs, path::PathBuf};

use hotki_world::WorldWindow;
use serde::Serialize;

use crate::{
    binding_watcher::{ACTIVATION_IDENT, ActivationOutcome, BindingWatcher},
    config,
    error::{Error, Result},
    server_drive,
    session::HotkiSession,
    suite::{CaseCtx, sanitize_slug},
    world,
};

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

        let session = HotkiSession::builder_from_env()?
            .with_config(&config_path)
            .with_logs(spec.with_logs)
            .spawn()?;

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
        let watcher = BindingWatcher::connect(&socket, state_ref.session.pid() as i32)?;
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
