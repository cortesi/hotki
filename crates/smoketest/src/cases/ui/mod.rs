//! UI-driven smoketest cases executed via the registry runner.

/// Activation polling and HUD-ready detection helpers.
mod activation;

use std::{cmp::Ordering, fs, thread, time::Instant};

use hotki_app_session::{
    session::{HotkiSession, HotkiSessionConfig},
    windows::{self as window_inspection, OwnedWindows, WindowSnapshot},
};
use hotki_protocol::{MsgToUI, Toggle};
use tracing::{debug, info};

use self::activation::{ActivationOutcome, BindingWatcher};
use crate::{
    config,
    error::{Error, Result},
    suite::{CaseCtx, LOG_TARGET, sanitize_slug},
};

/// Canonical activation chord identifier expected from the server.
pub const ACTIVATION_IDENT: &str = "shift+cmd+0";

/// Key that enters the demo submenu once the HUD is visible.
const UI_DEMO_ACTIVATE: &str = "t";
/// Key that triggers a shell-backed notification.
const UI_DEMO_NOTIFY: &str = "n";
/// Direct global key used by the notification-focused smoke case.
const NOTIFICATION_IDENT: &str = "shift+cmd+n";
/// Key that opens the Details window.
const UI_DEMO_DETAILS: &str = "d";
/// Key that opens the selector demo.
const UI_DEMO_SELECTOR: &str = "s";
/// Query key typed into the selector demo.
const UI_DEMO_SELECTOR_QUERY: &str = "a";
/// Key that selects the current selector item.
const UI_DEMO_SELECTOR_SELECT: &str = "return";
/// Key that exits the demo submenu.
const UI_DEMO_EXIT: &str = "esc";
/// Screen edge margin used by notification placement.
const NOTIFICATION_MARGIN_PX: f32 = 12.0;
/// Tolerance used for native window-edge comparisons.
const LAYOUT_SLOP_PX: f32 = 2.0;

/// Demo HUD mode applied in the generated Luau config.
#[derive(Clone, Copy)]
enum DemoHudMode {
    /// Full HUD mode anchored bottom-right.
    Hud,
    /// Compact mini HUD mode.
    Mini,
}

impl DemoHudMode {
    /// Render the Luau token used for the HUD mode.
    fn luau_value(self) -> &'static str {
        match self {
            Self::Hud => "\"hud\"",
            Self::Mini => "\"mini\"",
        }
    }
}

/// Build the standard demo config used by the UI smoketests.
fn demo_config() -> String {
    r#"
return function(menu, ctx)
  menu:submenu("shift+cmd+0", "activate", function(activate, inner)
    activate:submenu("t", "Tools", function(sub, subctx)
      sub:bind("n", "Notify", function(actx)
        actx:shell("echo notify", { ok_notify = "info", err_notify = "warn" })
      end, { stay = true })
      sub:bind("d", "Details", function(actx)
        actx:show_details("on")
      end, { stay = true })
      sub:bind("s", "Selector", function(actx)
        actx:select({
          title = "Pick Demo",
          placeholder = "Search...",
          items = { "Alpha", "Beta" },
          on_select = function(select_ctx, item, query)
            select_ctx:notify("info", "Selector", item.label .. ":" .. query)
          end,
          on_cancel = function(cancel_ctx)
            cancel_ctx:notify("warn", "Selector", "cancel")
          end,
        })
      end, { stay = true })
      sub:bind("esc", "Exit", function(actx)
        actx:exit()
      end, { hidden = true })
    end)
  end)
end
"#
    .to_string()
}

/// Build the sibling style file used by the UI smoketests.
fn demo_style(mode: DemoHudMode) -> String {
    format!(
        r#"
return {{
  hud = {{
      mode = {},
      pos = "se",
  }},
  notify = {{
      pos = "left",
  }},
}}
"#,
        mode.luau_value()
    )
}

/// Build a minimal default-theme config that triggers one notification.
fn notification_config() -> String {
    format!(
        r#"
return function(menu, ctx)
  menu:bind("{}", "Native Notification", function(actx)
    actx:shell("echo native notification", {{
      ok_notify = "info",
      err_notify = "warn",
    }})
  end)
end
"#,
        NOTIFICATION_IDENT
    )
}

/// Poll CoreGraphics until a notification window is visible.
fn wait_for_notification_windows(pid: i32, timeout_ms: u64) -> Result<Vec<WindowSnapshot>> {
    let start = Instant::now();
    let timeout = config::ms(timeout_ms);
    loop {
        let windows = OwnedWindows::new(pid as u32).notifications()?;
        if !windows.is_empty() {
            return Ok(windows);
        }
        if start.elapsed() >= timeout {
            let all_windows = OwnedWindows::new(pid as u32).list()?;
            return Err(Error::InvalidState(format!(
                "no notification window candidates; hotki windows: {}",
                describe_windows(&all_windows)
            )));
        }
        let Some(remaining) = timeout.checked_sub(start.elapsed()) else {
            continue;
        };
        thread::sleep(remaining.min(config::ms(config::INPUT_DELAYS.retry_delay_ms)));
    }
}

/// Wait until the active Hotki session owns a visible native notification window.
pub fn wait_for_notification_window(pid: i32, timeout_ms: u64) -> Result<()> {
    wait_for_notification_windows(pid, timeout_ms).map(|_| ())
}

/// Verify a notification candidate is aligned to the active display's top-right edge.
fn verify_notification_window(window: &WindowSnapshot) -> Result<()> {
    let displays = window_inspection::enumerate_displays()?;
    let display_id = window.display_id.ok_or_else(|| {
        Error::InvalidState(format!(
            "notification window {} missing display id",
            window.id
        ))
    })?;
    let display = displays
        .iter()
        .copied()
        .find(|display| display.id == display_id)
        .ok_or_else(|| {
            Error::InvalidState(format!(
                "display {display_id} missing during notification check"
            ))
        })?;
    let tolerance = NOTIFICATION_MARGIN_PX + LAYOUT_SLOP_PX;
    let right_delta = (display.max_x() - window.max_x()).abs();
    let expected_top = display.visible_y() + NOTIFICATION_MARGIN_PX;
    let top_delta = (window.y - expected_top).abs();
    if right_delta > tolerance {
        return Err(Error::InvalidState(format!(
            "notification right edge misaligned: window_max_x={} display_max_x={} delta={} \
             tolerance={} window={}",
            window.max_x(),
            display.max_x(),
            right_delta,
            tolerance,
            describe_window(window)
        )));
    }
    if top_delta > tolerance {
        return Err(Error::InvalidState(format!(
            "notification top edge misaligned: window_y={} expected_top={} visible_y={} \
             display_y={} delta={} tolerance={} window={}",
            window.y,
            expected_top,
            display.visible_y(),
            display.y(),
            top_delta,
            tolerance,
            describe_window(window)
        )));
    }
    let max_height = (display.visible_height() - 2.0 * NOTIFICATION_MARGIN_PX).max(1.0);
    if window.height > max_height + LAYOUT_SLOP_PX {
        return Err(Error::InvalidState(format!(
            "notification height exceeds max: height={} max={} window={}",
            window.height,
            max_height,
            describe_window(window)
        )));
    }
    Ok(())
}

/// Render a compact diagnostic for a list of windows.
fn describe_windows(windows: &[WindowSnapshot]) -> String {
    if windows.is_empty() {
        return "<none>".to_string();
    }
    windows
        .iter()
        .map(describe_window)
        .collect::<Vec<_>>()
        .join("; ")
}

/// Render a compact diagnostic for one window.
fn describe_window(window: &WindowSnapshot) -> String {
    format!(
        "title={} pid={} id={} display_id={:?} on_screen={} layer={} bounds=({}, {}, {}, {})",
        window.title,
        window.pid,
        window.id,
        window.display_id,
        window.is_on_screen,
        window.layer,
        window.x,
        window.y,
        window.width,
        window.height
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

/// Verify a default-position notification appears at the active display's right edge.
pub fn notifications(ctx: &mut CaseCtx<'_>) -> Result<()> {
    let mut session: Option<HotkiSession> = None;
    let mut notification_windows: Option<Vec<WindowSnapshot>> = None;

    ctx.setup(|ctx| {
        let config_path = ctx.scratch_path("notifications_config.luau");
        fs::write(&config_path, notification_config().as_bytes())?;
        let spawned = HotkiSession::spawn(
            HotkiSessionConfig::from_env()?
                .with_config(&config_path)
                .with_logs(true),
            ctx.run_budget(),
        )?;
        session = Some(spawned);
        Ok(())
    })?;

    ctx.action(|ctx| {
        let session_ref = session.as_mut().ok_or_else(|| {
            Error::InvalidState("notification case state missing during action".into())
        })?;
        {
            let driver = session_ref.driver_mut();
            driver.ensure_ready(ctx.remaining_ms()?)?;
            driver.wait_for_idents(&[NOTIFICATION_IDENT], ctx.remaining_ms()?)?;
            let cursor = driver.event_cursor()?;
            driver.inject_key(NOTIFICATION_IDENT, ctx.remaining_ms()?)?;
            driver.wait_for_message_since(
                cursor,
                ctx.remaining_ms()?,
                |msg| matches!(msg, MsgToUI::Notify { title, .. } if title == "Shell command"),
            )?;
        }

        let mut windows =
            wait_for_notification_windows(session_ref.pid() as i32, ctx.remaining_ms()?)?;
        windows.sort_by(|a, b| b.max_y().partial_cmp(&a.max_y()).unwrap_or(Ordering::Equal));
        let topmost = windows.first().ok_or_else(|| {
            Error::InvalidState("no notification window candidates were visible".into())
        })?;
        verify_notification_window(topmost)?;
        notification_windows = Some(windows);
        Ok(())
    })?;

    ctx.settle(|ctx| {
        let mut session_inner = session
            .take()
            .ok_or_else(|| Error::InvalidState("notification case state missing".into()))?;
        let windows_desc = notification_windows
            .as_ref()
            .map(|windows| describe_windows(windows))
            .unwrap_or_else(|| "<none>".to_string());
        ctx.log_event("notification_windows", &windows_desc);
        if let Err(err) = session_inner.shutdown() {
            debug!(
                ?err,
                "server driver shutdown returned error (expected on clean exit)"
            );
        }
        session_inner.kill_and_wait();
        Ok(())
    })?;

    Ok(())
}

/// Parameters describing a UI demo scenario.
struct UiCaseSpec {
    /// Registry slug recorded in artifacts.
    slug: &'static str,
    /// HUD mode injected into the generated Luau configuration.
    hud_mode: DemoHudMode,
    /// Whether to enable verbose logging for the child process.
    with_logs: bool,
}

impl UiCaseSpec {
    /// Render the Luau config used by this scenario.
    fn render_config(&self) -> String {
        demo_config()
    }

    /// Render the style file used by this scenario.
    fn render_style(&self) -> String {
        demo_style(self.hud_mode)
    }
}

/// Mutable state shared across UI case stages.
struct UiCaseState {
    /// Running hotki session launched for this demo.
    session: HotkiSession,
    /// Registry slug mirrored when logging diagnostics or creating scratch configs.
    slug: &'static str,
    /// Observed time in milliseconds until the HUD became visible.
    time_to_hud_ms: Option<u64>,
    /// Snapshot of HUD-related windows after activation.
    hud_windows: Option<Vec<WindowSnapshot>>,
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
        let filename = format!("{}_config.luau", sanitize_slug(spec.slug));
        let config_path = ctx.scratch_path(filename);
        let luau_config = spec.render_config();
        fs::write(&config_path, luau_config.as_bytes())?;
        fs::write(
            config_path.with_file_name("style.luau"),
            spec.render_style().as_bytes(),
        )?;

        let session = HotkiSession::spawn(
            HotkiSessionConfig::from_env()?
                .with_config(&config_path)
                .with_logs(spec.with_logs),
            ctx.run_budget(),
        )?;

        state = Some(UiCaseState {
            session,
            slug: spec.slug,
            time_to_hud_ms: None,
            hud_windows: None,
            hud_activation: None,
        });

        Ok(())
    })?;

    ctx.action(|ctx| {
        let state_ref = state
            .as_mut()
            .ok_or_else(|| Error::InvalidState("ui case state missing during action".into()))?;

        {
            let driver = state_ref.session.driver_mut();
            driver.ensure_ready(ctx.remaining_ms()?)?;
        }
        let watcher = BindingWatcher::new(state_ref.session.pid() as i32);
        let activation = {
            let driver = state_ref.session.driver_mut();
            watcher.activate_until_ready(
                driver,
                ACTIVATION_IDENT,
                &[UI_DEMO_ACTIVATE],
                ctx.remaining_ms()?,
            )?
        };
        if !activation.focus_event_seen() {
            info!(
                target: LOG_TARGET,
                pid = state_ref.session.pid(),
                "no focus-change server events observed during activation"
            );
        }

        {
            let driver = state_ref.session.driver_mut();
            driver.inject_key(UI_DEMO_ACTIVATE, ctx.remaining_ms()?)?;
            driver.wait_for_idents(&["n", "d", "s"], ctx.remaining_ms()?)?;
        }

        {
            let driver = state_ref.session.driver_mut();
            let cursor = driver.event_cursor()?;
            driver.inject_key(UI_DEMO_NOTIFY, ctx.remaining_ms()?)?;
            driver.wait_for_message_since(
                cursor,
                ctx.remaining_ms()?,
                |msg| matches!(msg, MsgToUI::Notify { title, .. } if title == "Shell command"),
            )?;
            let cursor = driver.event_cursor()?;
            driver.inject_key(UI_DEMO_DETAILS, ctx.remaining_ms()?)?;
            driver.wait_for_message_since(cursor, ctx.remaining_ms()?, |msg| {
                matches!(msg, MsgToUI::ShowDetails(Toggle::On))
            })?;
            let cursor = driver.event_cursor()?;
            driver.inject_key(UI_DEMO_SELECTOR, ctx.remaining_ms()?)?;
            driver.wait_for_message_since(cursor, ctx.remaining_ms()?, |msg| {
                matches!(msg, MsgToUI::SelectorUpdate(snapshot) if snapshot.title == "Pick Demo")
            })?;
            let cursor = driver.event_cursor()?;
            driver.inject_key(UI_DEMO_SELECTOR_QUERY, ctx.remaining_ms()?)?;
            driver.wait_for_message_since(
                cursor,
                ctx.remaining_ms()?,
                |msg| matches!(msg, MsgToUI::SelectorUpdate(snapshot) if snapshot.query == "a"),
            )?;
            let cursor = driver.event_cursor()?;
            driver.inject_key(UI_DEMO_SELECTOR_SELECT, ctx.remaining_ms()?)?;
            driver.wait_for_message_since(cursor, ctx.remaining_ms()?, |msg| {
                matches!(msg, MsgToUI::SelectorHide)
            })?;
            driver.wait_for_message_since(cursor, ctx.remaining_ms()?, |msg| {
                matches!(
                    msg,
                    MsgToUI::Notify { title, text, .. }
                        if title == "Selector" && text == "Alpha:a"
                )
            })?;
        }

        let hud_windows = state_ref.session.windows().list()?;

        state_ref.time_to_hud_ms = activation.hud_visible_ms();
        state_ref.hud_activation = Some(activation);
        state_ref.hud_windows = Some(hud_windows);

        after_activation(state_ref)?;

        state_ref
            .session
            .driver_mut()
            .inject_key(UI_DEMO_EXIT, ctx.remaining_ms()?)?;
        Ok(())
    })?;

    ctx.settle(|ctx| {
        let mut state_inner = state
            .take()
            .ok_or_else(|| Error::InvalidState("ui case state missing during settle".into()))?;

        let hud_windows_desc = state_inner
            .hud_windows
            .as_ref()
            .map(|wins| describe_windows(wins))
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
                "server driver shutdown returned error (expected on clean exit)"
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
        state.session.driver_mut().latest_hud()?.ok_or_else(|| {
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
        let windows = state.session.windows().list()?;
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
