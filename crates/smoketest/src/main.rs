use clap::Parser;

mod cli;
mod config;
mod error;
mod logging;
mod orchestrator;
mod process;
mod results;
mod runtime;
mod session;
mod test_runner;
mod tests;
mod ui_interaction;
mod util;
mod winhelper;

use cli::{Cli, Commands};
use error::print_hints;
use orchestrator::{heading, run_all_tests, run_preflight};
use tests::*;

// Re-export common result types
pub use results::{FocusOutcome, Summary, TestDetails, TestOutcome};

fn main() {
    let cli = Cli::parse();

    // Initialize logging if requested
    logging::init_logging(cli.logs);

    // For the helper command, skip the build and heading
    if matches!(cli.command, Commands::FocusWinHelper { .. }) {
        match cli.command {
            Commands::FocusWinHelper { title, time } => {
                if let Err(e) = winhelper::run_focus_winhelper(&title, time) {
                    eprintln!("focus-winhelper: ERROR: {}", e);
                    std::process::exit(2);
                }
            }
            _ => unreachable!(),
        }
        return;
    }

    // Build the hotki binary once at startup to avoid running against a stale build.
    heading("Building hotki");
    if let Err(e) = process::build_hotki_quiet() {
        eprintln!("Failed to build 'hotki' binary: {}", e);
        eprintln!("Try: cargo build -p hotki");
        std::process::exit(1);
    }

    match cli.command {
        Commands::Relay => {
            heading("Test: repeat-relay");
            repeat_relay(cli.duration)
        }
        Commands::Shell => {
            heading("Test: repeat-shell");
            repeat_shell(cli.duration)
        }
        Commands::Volume => {
            heading("Test: repeat-volume");
            // Volume can be slightly slower; keep a floor to reduce flakiness
            repeat_volume(std::cmp::max(
                cli.duration,
                config::MIN_VOLUME_TEST_DURATION_MS,
            ))
        }
        Commands::All => run_all_tests(cli.duration, cli.timeout),
        Commands::Raise => {
            heading("Test: raise");
            match raise::run_raise_test(cli.timeout, cli.logs) {
                Ok(()) => println!("raise: OK (raised by title twice)"),
                Err(e) => {
                    eprintln!("raise: ERROR: {}", e);
                    print_hints(&e);
                    std::process::exit(1);
                }
            }
        }
        Commands::Focus => {
            heading("Test: focus");
            match focus::run_focus_test(cli.timeout, cli.logs) {
                Ok(out) => {
                    println!(
                        "focus: OK (title='{}', pid={}, time_to_match_ms={})",
                        out.title, out.pid, out.elapsed_ms
                    );
                }
                Err(e) => {
                    eprintln!("focus: ERROR: {}", e);
                    print_hints(&e);
                    std::process::exit(1);
                }
            }
        }
        Commands::Hide => {
            heading("Test: hide");
            match hide::run_hide_test(cli.timeout, cli.logs) {
                Ok(()) => println!("hide: OK (toggle on/off roundtrip)"),
                Err(e) => {
                    eprintln!("hide: ERROR: {}", e);
                    print_hints(&e);
                    std::process::exit(1);
                }
            }
        }
        Commands::FocusWinHelper { .. } => {
            // Already handled above
            unreachable!()
        }
        Commands::Ui => {
            heading("Test: ui");
            match ui::run_ui_demo(cli.timeout) {
                Ok(sum) => {
                    println!(
                        "ui: OK (hud_seen={}, time_to_hud_ms={:?})",
                        sum.hud_seen, sum.time_to_hud_ms
                    );
                }
                Err(e) => {
                    eprintln!("ui: ERROR: {}", e);
                    print_hints(&e);
                    std::process::exit(1);
                }
            }
        }
        Commands::Screenshots { theme, dir } => {
            heading("Test: screenshots");
            match screenshot::run_screenshots(theme, dir, cli.timeout) {
                Ok(sum) => {
                    println!(
                        "screenshots: OK (hud_seen={}, time_to_hud_ms={:?})",
                        sum.hud_seen, sum.time_to_hud_ms
                    );
                }
                Err(e) => {
                    eprintln!("screenshots: ERROR: {}", e);
                    print_hints(&e);
                    std::process::exit(1);
                }
            }
        }
        Commands::Minui => {
            heading("Test: minui");
            match ui::run_minui_demo(cli.timeout) {
                Ok(sum) => {
                    println!(
                        "minui: OK (hud_seen={}, time_to_hud_ms={:?})",
                        sum.hud_seen, sum.time_to_hud_ms
                    );
                }
                Err(e) => {
                    eprintln!("minui: ERROR: {}", e);
                    print_hints(&e);
                    std::process::exit(1);
                }
            }
        }
        Commands::Preflight => {
            heading("Test: preflight");
            let ok = run_preflight();
            if !ok {
                std::process::exit(1);
            }
        }
    }
}
