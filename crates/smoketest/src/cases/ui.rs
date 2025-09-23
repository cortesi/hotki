//! UI-driven smoketest cases executed via the registry runner.

use std::fs;

use serde::Serialize;
use serde_json::json;

use crate::{
    config,
    error::{Error, Result},
    server_drive,
    session::HotkiSession,
    suite::CaseCtx,
    ui_interaction::{send_key, send_key_sequence},
    util, world,
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
    /// Registry slug mirrored in artifact file names.
    slug: &'static str,
    /// Observed time in milliseconds until the HUD became visible.
    time_to_hud_ms: Option<u64>,
    /// Snapshot of HUD-related windows after activation.
    hud_windows: Option<Vec<HudWindowSnapshot>>,
}

/// Core implementation that runs a HUD-focused UI smoketest.
fn run_ui_case(ctx: &mut CaseCtx<'_>, spec: &UiCaseSpec) -> Result<()> {
    let mut state: Option<UiCaseState> = None;

    ctx.setup(|stage| {
        let hotki_bin = util::resolve_hotki_bin().ok_or(Error::HotkiBinNotFound)?;
        let sanitized = spec.slug.replace('.', "_");
        let config_path = stage
            .artifacts_dir()
            .join(format!("{}_config.ron", sanitized));
        fs::write(&config_path, spec.ron_config)?;
        stage.record_artifact(&config_path);

        let session = HotkiSession::builder(hotki_bin)
            .with_config(&config_path)
            .with_logs(spec.with_logs)
            .spawn()?;

        state = Some(UiCaseState {
            session,
            slug: spec.slug,
            time_to_hud_ms: None,
            hud_windows: None,
        });

        Ok(())
    })?;

    ctx.action(|_| {
        let state_ref = state
            .as_mut()
            .ok_or_else(|| Error::InvalidState("ui case state missing during action".into()))?;

        let socket = state_ref.session.socket_path().to_string();
        server_drive::ensure_init(&socket, 3_000)?;
        let gate_ms = config::BINDING_GATES.default_ms * 3;
        server_drive::wait_for_idents(&["shift+cmd+0"], gate_ms)?;

        // Proactively trigger the activation chord before waiting for visibility.
        send_key_sequence(&["shift+cmd+0"])?;

        let hud_deadline = config::DEFAULTS.timeout_ms;
        let time_to_hud = state_ref.session.wait_for_hud_checked(hud_deadline)?;
        server_drive::wait_for_ident("shift+cmd+0", gate_ms)?;
        server_drive::wait_for_ident("t", gate_ms)?;
        send_key_sequence(UI_DEMO_SEQUENCE)?;

        let hud_windows = collect_hud_windows(state_ref.session.pid() as i32)?;
        if hud_windows.is_empty() {
            return Err(Error::InvalidState(
                "hud window not visible after activation".into(),
            ));
        }

        state_ref.time_to_hud_ms = Some(time_to_hud);
        state_ref.hud_windows = Some(hud_windows);

        // Close the HUD after capturing window diagnostics to leave the session cleanly.
        send_key("shift+cmd+0")?;
        Ok(())
    })?;

    ctx.settle(|stage| {
        let mut state_inner = state
            .take()
            .ok_or_else(|| Error::InvalidState("ui case state missing during settle".into()))?;

        let sanitized = state_inner.slug.replace('.', "_");
        let outcome_path = stage
            .artifacts_dir()
            .join(format!("{}_outcome.json", sanitized));
        let payload = json!({
            "slug": state_inner.slug,
            "hud_seen": state_inner.time_to_hud_ms.is_some(),
            "time_to_hud_ms": state_inner.time_to_hud_ms,
            "hud_windows": state_inner.hud_windows,
        });
        let mut data = serde_json::to_string_pretty(&payload)
            .map_err(|err| Error::InvalidState(format!("failed to serialize ui outcome: {err}")))?;
        data.push('\n');
        fs::write(&outcome_path, data)?;
        stage.record_artifact(&outcome_path);

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

impl From<mac_winops::WindowInfo> for HudWindowSnapshot {
    fn from(info: mac_winops::WindowInfo) -> Self {
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
