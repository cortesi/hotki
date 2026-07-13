#![allow(clippy::disallowed_methods)]
//! Smoketest binary for Hotki. Provides UI/HUD validation tests.
use clap::Parser;
use logging as logshared;
use tracing_subscriber::{fmt, prelude::*};

/// Scenario-specific smoketest cases for UI/HUD validation.
mod cases;
mod cli;
/// Smoketest case registry and runner.
mod suite;
/// Repo-local temp path helpers.
mod tmp_paths;
/// UI overlay to warn users to avoid typing during smoketests.
mod warn_overlay;

use std::{env, ffi::OsString, path::PathBuf, process::exit};

use cli::{Cli, Commands};
use error::print_hints;
use hotki_app_session::{config, error, process};
use warn_overlay::WARN_OVERLAY_STANDALONE_FLAG;

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

/// Build a runner configuration from the CLI flags.
fn runner_config(cli: &Cli) -> suite::RunnerConfig<'_> {
    suite::RunnerConfig {
        quiet: cli.quiet,
        warn_overlay: !cli.no_warn,
        run_budget_ms: cli.run_budget_ms,
        fail_fast: !cli.no_fail_fast,
        overlay_info: cli.info.as_deref(),
    }
}

/// Execute the supplied case slugs and exit on failure.
fn run_cases(cli: &Cli, slugs: &[&str]) {
    let config = runner_config(cli);
    if let Err(err) = suite::run_sequence(slugs, &config) {
        let label = slugs.join(", ");
        eprintln!("smoketest {label}: ERROR: {err}");
        print_hints(&err);
        exit(1);
    }
}

/// Execute a single registry case and exit on failure.
fn run_case_by_slug(cli: &Cli, slug: &str) {
    run_cases(cli, &[slug]);
}

fn main() {
    if maybe_run_warn_overlay_standalone() {
        return;
    }
    let cli = Cli::parse();

    init_tracing_from_cli(&cli);

    if matches!(&cli.command, Commands::List) {
        suite::print_case_catalog();
        return;
    }

    build_hotki_or_exit(&cli);

    dispatch_command(&cli);
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

/// Build the hotki app binary once up-front to avoid stale binaries.
fn build_hotki_or_exit(cli: &Cli) {
    if !cli.quiet {
        heading("Building hotki-app");
    }
    if let Err(e) = process::build_hotki_app() {
        eprintln!("Failed to build 'hotki-app' binary: {}", e);
        eprintln!("Try: cargo build -p hotki-app --bin hotki-app");
        exit(1);
    }
}

/// Dispatch to the concrete smoketest command handlers.
fn dispatch_command(cli: &Cli) {
    let total = cli.repeat;
    for iteration in 1..=total {
        if total > 1 && !cli.quiet {
            heading(&format!("Run {iteration}/{total}"));
        }
        dispatch_command_once(cli);
    }
}

/// Execute one iteration of the smoketest command dispatch.
fn dispatch_command_once(cli: &Cli) {
    if let Some(slug) = cli.command.case_slug() {
        run_case_by_slug(cli, slug);
        return;
    }

    match &cli.command {
        Commands::All => {
            let config = runner_config(cli);
            if let Err(err) = suite::run_all(&config) {
                eprintln!("smoketest all failed: {err}");
                exit(1);
            }
        }
        Commands::Seq { tests } => {
            let slugs: Vec<&str> = tests.iter().map(String::as_str).collect();
            run_cases(cli, &slugs);
        }
        Commands::List => suite::print_case_catalog(),
        _ => {
            // Commands without a case_info entry are handled here or are errors.
        }
    }
}
