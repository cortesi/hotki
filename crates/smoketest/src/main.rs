use clap::Parser;

mod cli;
mod config;
mod error;
mod logging;
mod orchestrator;
mod proc_registry;
mod process;
mod results;
mod runtime;
mod server_drive;
mod session;
mod test_runner;
mod tests;
mod ui_interaction;
mod util;
mod winhelper;

use cli::{Cli, Commands, FsState};
use error::print_hints;
use hotki_protocol::Toggle;
use orchestrator::{heading, run_all_tests};
use std::sync::mpsc;
use std::time::Duration;
use tests::*;

fn run_with_watchdog<F, T>(name: &str, timeout_ms: u64, f: F) -> T
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let out = f();
        let _ = tx.send(out);
    });
    match rx.recv_timeout(Duration::from_millis(timeout_ms)) {
        Ok(v) => v,
        Err(_) => {
            eprintln!(
                "ERROR: smoketest watchdog timeout ({} ms) in {} — force exiting",
                timeout_ms, name
            );
            crate::proc_registry::kill_all();
            std::process::exit(2);
        }
    }
}

// Re-export common result types
pub use results::{FocusOutcome, Summary, TestDetails, TestOutcome};

fn main() {
    let cli = Cli::parse();

    // Initialize logging if requested
    logging::init_logging(cli.logs);

    // For the helper command, skip permission/build checks and heading
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

    // Enforce required permissions for all smoketests.
    let p = permissions::check_permissions();
    if !p.accessibility_ok || !p.input_ok {
        eprintln!(
            "ERROR: required permissions missing (accessibility={}, input_monitoring={})",
            p.accessibility_ok, p.input_ok
        );
        eprintln!(
            "Grant Accessibility and Input Monitoring to your terminal under System Settings → Privacy & Security."
        );
        std::process::exit(1);
    }

    // Screenshots require Screen Recording; fail fast with a clear error if not granted.
    if matches!(cli.command, Commands::Screenshots { .. }) {
        let sr_ok = screen_recording_ok();
        println!("screen_recording: {}", sr_ok);
        if !sr_ok {
            eprintln!("ERROR: Screen Recording permission is required for screenshots");
            eprintln!(
                "Grant Screen Recording permission to your terminal under System Settings → Privacy & Security → Screen Recording."
            );
            std::process::exit(1);
        }
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
            let duration = cli.duration;
            run_with_watchdog("repeat-relay", cli.timeout, move || repeat_relay(duration));
        }
        Commands::Shell => {
            heading("Test: repeat-shell");
            let duration = cli.duration;
            run_with_watchdog("repeat-shell", cli.timeout, move || repeat_shell(duration));
        }
        Commands::Volume => {
            heading("Test: repeat-volume");
            // Volume can be slightly slower; keep a floor to reduce flakiness
            let duration = std::cmp::max(cli.duration, config::MIN_VOLUME_TEST_DURATION_MS);
            run_with_watchdog("repeat-volume", cli.timeout, move || {
                repeat_volume(duration)
            });
        }
        Commands::All => run_all_tests(cli.duration, cli.timeout, cli.logs),
        Commands::Seq { tests } => {
            orchestrator::run_sequence_tests(&tests, cli.duration, cli.timeout, cli.logs)
        }
        Commands::Raise => {
            heading("Test: raise");
            let timeout = cli.timeout;
            let logs = cli.logs;
            match run_with_watchdog("raise", timeout, move || {
                raise::run_raise_test(timeout, logs)
            }) {
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
            let timeout = cli.timeout;
            let logs = cli.logs;
            match run_with_watchdog("focus", timeout, move || {
                focus::run_focus_test(timeout, logs)
            }) {
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
            let timeout = cli.timeout;
            let logs = cli.logs;
            match run_with_watchdog("hide", timeout, move || hide::run_hide_test(timeout, logs)) {
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
            let timeout = cli.timeout;
            match run_with_watchdog("ui", timeout, move || ui::run_ui_demo(timeout)) {
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
            let timeout = cli.timeout;
            match run_with_watchdog("screenshots", timeout, move || {
                screenshot::run_screenshots(theme, dir, timeout)
            }) {
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
            let timeout = cli.timeout;
            match run_with_watchdog("minui", timeout, move || ui::run_minui_demo(timeout)) {
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
        Commands::Fullscreen { state, native } => {
            heading("Test: fullscreen");
            let toggle = match state {
                FsState::Toggle => Toggle::Toggle,
                FsState::On => Toggle::On,
                FsState::Off => Toggle::Off,
            };
            let timeout = cli.timeout;
            let logs = cli.logs;
            match run_with_watchdog("fullscreen", timeout, move || {
                tests::fullscreen::run_fullscreen_test(timeout, logs, toggle, native)
            }) {
                Ok(()) => println!("fullscreen: OK (toggled non-native fullscreen)"),
                Err(e) => {
                    eprintln!("fullscreen: ERROR: {}", e);
                    print_hints(&e);
                    std::process::exit(1);
                }
            }
        } // Preflight smoketest removed.
    }
}

// Try capturing a 1×1 rectangle to test Screen Recording permission quickly.
fn screen_recording_ok() -> bool {
    use std::ffi::OsStr;
    let tmp = std::env::temp_dir().join(format!(
        "hotki-smoketest-preflight-{}-{}.png",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let status = std::process::Command::new("screencapture")
        .args([
            OsStr::new("-x"),
            OsStr::new("-R"),
            OsStr::new("0,0,1,1"),
            tmp.as_os_str(),
        ])
        .status();
    let ok = status.map(|s| s.success()).unwrap_or(false);
    let _ = std::fs::remove_file(&tmp);
    ok
}
