#![allow(clippy::disallowed_methods)]
//! Smoketest binary for Hotki. Provides repeat and UI validation helpers.
use clap::Parser;
use logging as logshared;
use tracing_subscriber::{fmt, prelude::*};

/// Event-driven binding watchers that keep the HUD responsive.
mod binding_watcher;
/// Scenario-specific smoketest cases and mimic harness helpers.
mod cases;
mod cli;
mod config;
/// Error definitions and hint helpers used by smoketest.
mod error;
/// Focus guards that reconcile world and AX views for helper windows.
mod focus_guard;
/// Shared helper utilities for new smoketest cases.
mod helpers;
mod process;
/// RPC driving helpers against the running server.
mod server_drive;
/// Session management for launching and controlling hotki.
mod session;
/// Mission Control capture helpers.
mod space_probe;
/// Smoketest case registry and runner.
mod suite;
/// UI overlay to warn users to avoid typing during smoketests.
mod warn_overlay;
/// Helper window for UI-driven tests and animations.
/// World snapshot helpers backed by hotki-world.
mod world;

use std::{
    env,
    ffi::OsString,
    path::{Path, PathBuf},
    process::exit,
    sync::atomic::{AtomicBool, Ordering},
};

use cli::{Cli, Commands};
use error::print_hints;
use hotki_world::mimic::run_focus_winhelper;
use process::WARN_OVERLAY_STANDALONE_FLAG;
use suite::CaseRunOpts;
/// Tracks whether hotki was already built during this smoketest invocation.
static HOTKI_BUILT: AtomicBool = AtomicBool::new(false);

/// Print a standardized heading for a smoketest section.
pub(crate) fn heading(title: &str) {
    println!("\n==> {}", title);
}

/// Arguments supported by the standalone warn overlay helper.
#[derive(Parser, Debug)]
struct WarnOverlayArgs {
    /// Optional path from which the overlay reads status text to display
    #[arg(long)]
    status_path: Option<PathBuf>,
    /// Optional path from which the overlay reads info text to display
    #[arg(long)]
    info_path: Option<PathBuf>,
}

/// Run the warn overlay helper when invoked directly and return whether it handled execution.
fn maybe_run_warn_overlay_standalone() -> bool {
    let mut args = env::args_os();
    if args.next().is_none() {
        return false;
    }
    let Some(flag) = args.next() else {
        return false;
    };
    if flag != WARN_OVERLAY_STANDALONE_FLAG {
        return false;
    }
    let mut overlay_args_input: Vec<OsString> = Vec::with_capacity(1);
    overlay_args_input.push(OsString::from("warn-overlay"));
    overlay_args_input.extend(args);
    let overlay_args = WarnOverlayArgs::parse_from(overlay_args_input);
    if let Err(err) =
        warn_overlay::run_warn_overlay(overlay_args.status_path, overlay_args.info_path)
    {
        eprintln!("warn-overlay: ERROR: {}", err);
        exit(2);
    }
    true
}

/// Build a runner configuration from the CLI flags with optional overrides applied.
fn runner_config<'a>(cli: &'a Cli, opts: &CaseRunOpts) -> suite::RunnerConfig<'a> {
    let mut config = suite::RunnerConfig {
        quiet: cli.quiet,
        warn_overlay: !cli.no_warn,
        base_timeout_ms: cli.timeout,
        fail_fast: !cli.no_fail_fast,
        overlay_info: cli.info.as_deref(),
    };
    if let Some(warn_overlay) = opts.warn_overlay {
        config.warn_overlay = warn_overlay;
    }
    if let Some(fail_fast) = opts.fail_fast {
        config.fail_fast = fail_fast;
    }
    config
}

/// Execute the supplied case slugs and exit on failure.
fn run_cases(cli: &Cli, slugs: &[&str], opts: CaseRunOpts) {
    let config = runner_config(cli, &opts);
    if let Err(err) = suite::run_sequence(slugs, &config) {
        let label = slugs.join(", ");
        eprintln!("smoketest {label}: ERROR: {err}");
        print_hints(&err);
        exit(1);
    }
}

/// Execute a single registry case and exit on failure.
fn run_case_by_slug(cli: &Cli, slug: &str, opts: CaseRunOpts) {
    run_cases(cli, &[slug], opts);
}

fn main() {
    if maybe_run_warn_overlay_standalone() {
        return;
    }
    let cli = Cli::parse();

    init_tracing_from_cli(&cli);

    if handle_helper_commands_early(&cli) {
        return;
    }

    let perms = permissions::check_permissions();
    let fake_mode = (!perms.accessibility_ok || !perms.input_ok) && env::var_os("CI").is_some();
    if fake_mode && !cli.quiet {
        println!(
            "smoketest: Accessibility/Input permissions missing; running fake placement smoke"
        );
    }

    enforce_permissions_or_exit(perms, fake_mode);
    build_hotki_or_exit(&cli);

    dispatch_command(&cli, fake_mode);
}

/// Initialize tracing/logging according to CLI flags and defaults.
fn init_tracing_from_cli(cli: &Cli) {
    let spec = if cli.quiet {
        logshared::level_spec_for("warn")
    } else {
        logshared::compute_spec(
            cli.log.trace,
            cli.log.debug,
            cli.log.log_level.as_deref(),
            cli.log.log_filter.as_deref(),
        )
    };
    let env_filter = logshared::env_filter_from_spec(&spec);
    let _init_res = tracing_subscriber::registry()
        .with(env_filter)
        .with(fmt::layer().without_time())
        .try_init();
}

/// Handle helper subcommands that bypass standard checks. Returns true if handled.
fn handle_helper_commands_early(cli: &Cli) -> bool {
    if let Commands::FocusWinHelper {
        title,
        time,
        delay_setframe_ms,
        delay_apply_ms,
        tween_ms,
        apply_target,
        apply_grid,
        slot,
        grid,
        size,
        pos,
        label_text,
        min_size,
        step_size,
        start_minimized,
        start_zoomed,
        panel_nonmovable,
        non_resizable,
        attach_sheet,
    } = &cli.command
    {
        let grid_tuple = grid
            .as_ref()
            .and_then(|v| (v.len() == 4).then(|| (v[0], v[1], v[2], v[3])));
        let size_tuple = size
            .as_ref()
            .and_then(|v| (v.len() == 2).then(|| (v[0], v[1])));
        let pos_tuple = pos
            .as_ref()
            .and_then(|v| (v.len() == 2).then(|| (v[0], v[1])));
        let step_size_tuple = step_size
            .as_ref()
            .and_then(|v| (v.len() == 2).then(|| (v[0], v[1])));
        let min_size_tuple = min_size
            .as_ref()
            .and_then(|v| (v.len() == 2).then(|| (v[0], v[1])));
        let apply_target_tuple = apply_target
            .as_ref()
            .and_then(|v| (v.len() == 4).then(|| (v[0], v[1], v[2], v[3])));
        let apply_grid_tuple = apply_grid
            .as_ref()
            .and_then(|v| (v.len() == 4).then(|| (v[0], v[1], v[2], v[3])));

        for iteration in 1..=cli.repeat {
            if cli.repeat > 1 && !cli.quiet {
                heading(&format!("Run {iteration}/{}", cli.repeat));
            }
            if let Err(e) = run_focus_winhelper(
                title,
                *time,
                delay_setframe_ms.unwrap_or(0),
                delay_apply_ms.unwrap_or(0),
                tween_ms.unwrap_or(0),
                apply_target_tuple,
                apply_grid_tuple,
                *slot,
                grid_tuple,
                size_tuple,
                pos_tuple,
                label_text.clone(),
                min_size_tuple,
                step_size_tuple,
                *start_minimized,
                *start_zoomed,
                *panel_nonmovable,
                *non_resizable,
                *attach_sheet,
            ) {
                eprintln!("focus-winhelper: ERROR: {}", e);
                exit(2);
            }
        }
        return true;
    }
    false
}

/// Ensure required macOS permissions are granted; exit with a helpful message if not.
fn enforce_permissions_or_exit(perms: permissions::PermissionsStatus, fake_mode: bool) {
    if fake_mode {
        return;
    }
    if !perms.accessibility_ok || !perms.input_ok {
        eprintln!(
            "ERROR: required permissions missing (accessibility={}, input_monitoring={})",
            perms.accessibility_ok, perms.input_ok
        );
        eprintln!(
            "Grant Accessibility and Input Monitoring to your terminal under System Settings -> Privacy & Security."
        );
        exit(1);
    }
}

/// Build the hotki binary once up-front to avoid stale binaries.
fn build_hotki_or_exit(cli: &Cli) {
    if env::var_os("HOTKI_SKIP_BUILD").is_some() || HOTKI_BUILT.load(Ordering::SeqCst) {
        return;
    }
    if !cli.quiet {
        heading("Building hotki");
    }
    if let Err(e) = process::build_hotki_quiet() {
        eprintln!("Failed to build 'hotki' binary: {}", e);
        eprintln!("Try: cargo build -p hotki");
        exit(1);
    }
    HOTKI_BUILT.store(true, Ordering::SeqCst);
}

/// Dispatch to the concrete smoketest command handlers.
fn dispatch_command(cli: &Cli, fake_mode: bool) {
    let total = cli.repeat;
    for iteration in 1..=total {
        if total > 1 && !cli.quiet {
            heading(&format!("Run {iteration}/{total}"));
        }
        dispatch_command_once(cli, fake_mode);
    }
}

/// Execute one iteration of the smoketest command dispatch.
fn dispatch_command_once(cli: &Cli, fake_mode: bool) {
    if let Some((slug, opts)) = cli.command.case_info(fake_mode) {
        run_case_by_slug(cli, slug, opts);
        return;
    }

    match &cli.command {
        Commands::All => {
            if fake_mode {
                let fake_opts = CaseRunOpts {
                    warn_overlay: Some(false),
                    fail_fast: Some(true),
                };
                run_case_by_slug(cli, "place.fake.adapter", fake_opts);
            } else {
                let config = runner_config(cli, &CaseRunOpts::default());
                if let Err(err) = suite::run_all(&config) {
                    eprintln!("smoketest all failed: {err}");
                    exit(1);
                }
            }
        }
        Commands::Seq { tests } => {
            let slugs: Vec<&str> = tests.iter().map(|test| test.slug()).collect();
            run_cases(cli, &slugs, CaseRunOpts::default());
        }
        Commands::SpaceProbe {
            samples,
            interval_ms,
            output,
        } => handle_space_probe(cli, *samples, *interval_ms, output.as_deref()),
        _ => {
            // Commands without a case_info entry are handled here or are errors.
            // FocusWinHelper is handled in handle_helper_commands_early.
        }
    }
}

/// Invoke the Mission Control space probe helper.
fn handle_space_probe(cli: &Cli, samples: u32, interval_ms: u64, output: Option<&Path>) {
    if !cli.quiet {
        heading("space-probe");
    }
    if let Err(e) = space_probe::run(samples, interval_ms, output, cli.quiet) {
        eprintln!("space-probe: ERROR: {e}");
        print_hints(&e);
        exit(1);
    }
}
