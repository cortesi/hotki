//! Capture the checked-in Hotki UI gallery through an owned app session.

use std::{
    env, fs,
    path::{Path, PathBuf},
    process::ExitCode,
    time::Duration,
};

use clap::Parser;
use hotki_app_session::{
    config::RunBudget,
    error::{Error, Result},
    session::{HotkiSession, HotkiSessionConfig},
    windows::{OwnedWindows, WindowSnapshot},
};
use hotki_protocol::{MsgToUI, NotifyKind};

/// Checked-in behavior fixture used to render the gallery.
const DEFAULT_CONFIG_PATH: &str = "crates/hotki-shots/fixtures/config.luau";
/// Window title used by the HUD viewport.
const HUD_TITLE: &str = "Hotki HUD";
/// Window title used by notification viewports.
const NOTIFICATION_TITLE: &str = "Hotki Notification";
/// Window title used by the selector viewport.
const SELECTOR_TITLE: &str = "Hotki Selector";

/// One notification gallery capture requested from the fixture.
#[derive(Clone, Copy)]
struct NotificationShot {
    /// Bound key that creates the notification.
    ident: &'static str,
    /// Output filename without extension.
    name: &'static str,
    /// Protocol kind expected from the server.
    kind: NotifyKind,
}

#[derive(Parser, Debug)]
#[command(
    name = "hotki-shots",
    about = "Capture Hotki HUD and notifications as PNGs",
    version
)]
struct Cli {
    /// Output directory for PNG files.
    #[arg(long)]
    dir: PathBuf,
    /// Hotki config to drive while capturing screenshots.
    #[arg(long)]
    config: Option<PathBuf>,
    /// Total wall-clock budget in milliseconds for the complete capture session.
    #[arg(long, default_value_t = 10_000)]
    timeout: u64,
    /// Enable logging for the spawned Hotki process.
    #[arg(long, default_value_t = false)]
    logs: bool,
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("hotki-shots: ERROR: {error}");
            ExitCode::FAILURE
        }
    }
}

/// Drive the screenshot fixture through semantic server events and PID-owned windows.
fn run(cli: Cli) -> Result<()> {
    let config_path = resolve_config_path(cli.config.as_deref())?;
    fs::create_dir_all(&cli.dir)?;
    let run_budget = RunBudget::new(cli.timeout);
    let mut session = HotkiSession::spawn(
        HotkiSessionConfig::from_env()?
            .with_config(config_path)
            .with_logs(cli.logs),
        run_budget,
    )?;
    let windows = session.windows();

    show_hud(&mut session, windows, run_budget)?;
    capture_title(windows, HUD_TITLE, &cli.dir, "hud", run_budget)?;

    session
        .driver_mut()
        .inject_key("n", remaining_ms(run_budget, "opening notification menu")?)
        .map_err(Error::from)?;
    session
        .driver_mut()
        .wait_for_idents(
            &["s", "i", "w", "e", "c", "p"],
            remaining_ms(run_budget, "notification bindings")?,
        )
        .map_err(Error::from)?;

    for shot in [
        NotificationShot {
            ident: "s",
            name: "notify_success",
            kind: NotifyKind::Success,
        },
        NotificationShot {
            ident: "i",
            name: "notify_info",
            kind: NotifyKind::Info,
        },
        NotificationShot {
            ident: "w",
            name: "notify_warning",
            kind: NotifyKind::Warn,
        },
        NotificationShot {
            ident: "e",
            name: "notify_error",
            kind: NotifyKind::Error,
        },
    ] {
        capture_notification(&mut session, windows, &cli.dir, shot, run_budget)?;
    }

    capture_selector(&mut session, windows, &cli.dir, run_budget)?;
    session.shutdown()?;
    println!("screenshots: OK (dir={})", cli.dir.display());
    Ok(())
}

/// Resolve and validate the behavior fixture path.
fn resolve_config_path(config: Option<&Path>) -> Result<PathBuf> {
    let current_dir = env::current_dir()?;
    let path = match config {
        Some(path) if path.is_absolute() => path.to_path_buf(),
        Some(path) => current_dir.join(path),
        None => current_dir.join(DEFAULT_CONFIG_PATH),
    };
    if !path.is_file() {
        return Err(Error::InvalidState(format!(
            "config not found: {}",
            path.display()
        )));
    }
    Ok(path)
}

/// Activate the HUD and require both a visible protocol snapshot and PID-owned window.
fn show_hud(session: &mut HotkiSession, windows: OwnedWindows, budget: RunBudget) -> Result<()> {
    let driver = session.driver_mut();
    driver
        .wait_for_idents(&["shift+cmd+0"], remaining_ms(budget, "HUD binding")?)
        .map_err(Error::from)?;
    let cursor = driver.event_cursor().map_err(Error::from)?;
    driver
        .inject_key("shift+cmd+0", remaining_ms(budget, "HUD activation")?)
        .map_err(Error::from)?;
    driver
        .wait_for_message_since(
            cursor,
            remaining_ms(budget, "visible HUD event")?,
            |message| matches!(message, MsgToUI::HudUpdate { hud, .. } if hud.visible),
        )
        .map_err(Error::from)?;
    windows.wait_for_title(HUD_TITLE, remaining_ms(budget, "HUD window")?)?;
    session.wait_for_hud_frame()?;
    Ok(())
}

/// Capture one notification kind and wait for its explicit clear event and window removal.
fn capture_notification(
    session: &mut HotkiSession,
    windows: OwnedWindows,
    output_dir: &Path,
    shot: NotificationShot,
    budget: RunBudget,
) -> Result<()> {
    let cursor = session.driver_mut().event_cursor().map_err(Error::from)?;
    session
        .driver_mut()
        .inject_key(shot.ident, remaining_ms(budget, "notification activation")?)
        .map_err(Error::from)?;
    session
        .driver_mut()
        .wait_for_message_since(
            cursor,
            remaining_ms(budget, "notification event")?,
            |message| {
            matches!(message, MsgToUI::Notify { kind: observed, .. } if *observed == shot.kind)
            },
        )
        .map_err(Error::from)?;
    session.wait_for_notification_frame(shot.kind)?;
    capture_title(windows, NOTIFICATION_TITLE, output_dir, shot.name, budget)?;

    let cursor = session.driver_mut().event_cursor().map_err(Error::from)?;
    session
        .driver_mut()
        .inject_key("c", remaining_ms(budget, "notification clear")?)
        .map_err(Error::from)?;
    session
        .driver_mut()
        .wait_for_message_since(
            cursor,
            remaining_ms(budget, "notification clear event")?,
            |message| matches!(message, MsgToUI::ClearNotifications),
        )
        .map_err(Error::from)?;
    windows.wait_until_closed(
        NOTIFICATION_TITLE,
        remaining_ms(budget, "notification window close")?,
    )
}

/// Open, query, capture, and close the selector through its protocol snapshots.
fn capture_selector(
    session: &mut HotkiSession,
    windows: OwnedWindows,
    output_dir: &Path,
    budget: RunBudget,
) -> Result<()> {
    wait_for_selector_query(session, "p", "", budget)?;
    let mut query = String::new();
    for ident in ["c", "a", "l"] {
        query.push_str(ident);
        wait_for_selector_query(session, ident, &query, budget)?;
    }
    session.wait_for_selector_frame(&query)?;
    capture_title(windows, SELECTOR_TITLE, output_dir, "selector", budget)?;

    let cursor = session.driver_mut().event_cursor().map_err(Error::from)?;
    session
        .driver_mut()
        .inject_key("esc", remaining_ms(budget, "selector close")?)
        .map_err(Error::from)?;
    session
        .driver_mut()
        .wait_for_message_since(
            cursor,
            remaining_ms(budget, "selector close event")?,
            |message| matches!(message, MsgToUI::SelectorHide),
        )
        .map_err(Error::from)?;
    windows.wait_until_closed(
        SELECTOR_TITLE,
        remaining_ms(budget, "selector window close")?,
    )
}

/// Inject one selector key and wait until the server publishes the expected query.
fn wait_for_selector_query(
    session: &mut HotkiSession,
    ident: &str,
    expected_query: &str,
    budget: RunBudget,
) -> Result<()> {
    let cursor = session.driver_mut().event_cursor().map_err(Error::from)?;
    session
        .driver_mut()
        .inject_key(ident, remaining_ms(budget, "selector input")?)
        .map_err(Error::from)?;
    session
        .driver_mut()
        .wait_for_message_since(
            cursor,
            remaining_ms(budget, "selector query event")?,
            |message| {
                matches!(
                    message,
                    MsgToUI::SelectorUpdate(snapshot) if snapshot.query == expected_query
                )
            },
        )
        .map_err(Error::from)?;
    Ok(())
}

/// Capture an exact PID-owned titled window to a deterministic gallery filename.
fn capture_title(
    windows: OwnedWindows,
    title: &str,
    output_dir: &Path,
    name: &str,
    budget: RunBudget,
) -> Result<WindowSnapshot> {
    let window = windows.wait_for_title(title, remaining_ms(budget, "window capture")?)?;
    window.capture_png(
        &output_dir.join(format!("{name}.png")),
        remaining(budget, "screencapture")?,
    )?;
    Ok(window)
}

/// Return the shared session's remaining whole-millisecond allowance.
fn remaining_ms(budget: RunBudget, operation: &str) -> Result<u64> {
    let remaining = remaining(budget, operation)?;
    let millis = remaining.as_millis().try_into().unwrap_or(u64::MAX);
    Ok(millis.max(1))
}

/// Return the shared session's remaining duration.
fn remaining(budget: RunBudget, operation: &str) -> Result<Duration> {
    budget.remaining().ok_or_else(|| {
        Error::InvalidState(format!(
            "screenshot run budget exhausted during {operation} ({} ms total)",
            budget.total_ms()
        ))
    })
}
