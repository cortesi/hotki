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
mod warn_overlay;
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

// Some tests (e.g., those that create a winit/Tao EventLoop) must run on the
// main thread on macOS. This variant keeps the test on the main thread and
// enforces a timeout via a background watchdog.
fn run_on_main_with_watchdog<F, T>(name: &str, timeout_ms: u64, f: F) -> T
where
    F: FnOnce() -> T,
{
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::thread;
    use std::time::{Duration, Instant};

    let canceled = Arc::new(AtomicBool::new(false));
    let canceled_flag = canceled.clone();
    let name_owned = name.to_string();
    let watchdog = thread::spawn(move || {
        let start = Instant::now();
        loop {
            if canceled_flag.load(Ordering::SeqCst) {
                return;
            }
            if start.elapsed() >= Duration::from_millis(timeout_ms) {
                eprintln!(
                    "ERROR: smoketest watchdog timeout ({} ms) in {} — force exiting",
                    timeout_ms, name_owned
                );
                crate::proc_registry::kill_all();
                std::process::exit(2);
            }
            thread::sleep(Duration::from_millis(25));
        }
    });

    // Run the test body on the main thread
    let out = f();
    canceled.store(true, Ordering::SeqCst);
    let _ = watchdog.join();
    out
}

// Re-export common result types
pub use results::{FocusOutcome, Summary, TestDetails, TestOutcome};

fn main() {
    let cli = Cli::parse();

    // Initialize logging according to flags
    logging::init_for(cli.logs, cli.quiet);

    // For helper commands, skip permission/build checks and heading
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
    if matches!(cli.command, Commands::WarnOverlay) {
        match warn_overlay::run_warn_overlay() {
            Ok(()) => {}
            Err(e) => {
                eprintln!("warn-overlay: ERROR: {}", e);
                std::process::exit(2);
            }
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

    // Screenshots extracted to separate tool: hotki-shots

    // Build the hotki binary once at startup to avoid running against a stale build.
    if !cli.quiet {
        heading("Building hotki");
    }
    if let Err(e) = process::build_hotki_quiet() {
        eprintln!("Failed to build 'hotki' binary: {}", e);
        eprintln!("Try: cargo build -p hotki");
        std::process::exit(1);
    }

    match cli.command {
        Commands::Relay => {
            if !cli.quiet {
                heading("Test: repeat-relay");
            }
            let duration = cli.duration;
            let mut overlay = None;
            if !cli.no_warn {
                overlay = crate::process::start_warn_overlay_with_delay();
            }
            // repeat‑relay opens a winit EventLoop; it must run on the main thread.
            run_on_main_with_watchdog("repeat-relay", cli.timeout, move || repeat_relay(duration));
            if let Some(mut o) = overlay {
                let _ = o.kill_and_wait();
            }
        }
        Commands::Shell => {
            if !cli.quiet {
                heading("Test: repeat-shell");
            }
            let duration = cli.duration;
            let mut overlay = None;
            if !cli.no_warn {
                overlay = crate::process::start_warn_overlay_with_delay();
            }
            run_with_watchdog("repeat-shell", cli.timeout, move || repeat_shell(duration));
            if let Some(mut o) = overlay {
                let _ = o.kill_and_wait();
            }
        }
        Commands::Volume => {
            if !cli.quiet {
                heading("Test: repeat-volume");
            }
            // Volume can be slightly slower; keep a floor to reduce flakiness
            let duration = std::cmp::max(cli.duration, config::MIN_VOLUME_TEST_DURATION_MS);
            let mut overlay = None;
            if !cli.no_warn {
                overlay = crate::process::start_warn_overlay_with_delay();
            }
            run_with_watchdog("repeat-volume", cli.timeout, move || {
                repeat_volume(duration)
            });
            if let Some(mut o) = overlay {
                let _ = o.kill_and_wait();
            }
        }
        Commands::All => run_all_tests(cli.duration, cli.timeout, cli.logs, !cli.no_warn),
        Commands::Seq { tests } => {
            orchestrator::run_sequence_tests(&tests, cli.duration, cli.timeout, cli.logs)
        }
        Commands::Raise => {
            if !cli.quiet {
                heading("Test: raise");
            }
            let timeout = cli.timeout;
            let logs = cli.logs;
            let mut overlay = None;
            if !cli.no_warn {
                overlay = crate::process::start_warn_overlay_with_delay();
            }
            match run_with_watchdog("raise", timeout, move || {
                raise::run_raise_test(timeout, logs)
            }) {
                Ok(()) => {
                    if !cli.quiet {
                        println!("raise: OK (raised by title twice)")
                    }
                }
                Err(e) => {
                    eprintln!("raise: ERROR: {}", e);
                    print_hints(&e);
                    if let Some(mut o) = overlay {
                        let _ = o.kill_and_wait();
                    }
                    std::process::exit(1);
                }
            }
            if let Some(mut o) = overlay {
                let _ = o.kill_and_wait();
            }
        }
        Commands::Focus => {
            if !cli.quiet {
                heading("Test: focus");
            }
            let timeout = cli.timeout;
            let logs = cli.logs;
            let mut overlay = None;
            if !cli.no_warn {
                overlay = crate::process::start_warn_overlay_with_delay();
            }
            match run_with_watchdog("focus", timeout, move || {
                focus::run_focus_test(timeout, logs)
            }) {
                Ok(out) => {
                    if !cli.quiet {
                        println!(
                            "focus: OK (title='{}', pid={}, time_to_match_ms={})",
                            out.title, out.pid, out.elapsed_ms
                        );
                    }
                }
                Err(e) => {
                    eprintln!("focus: ERROR: {}", e);
                    print_hints(&e);
                    if let Some(mut o) = overlay {
                        let _ = o.kill_and_wait();
                    }
                    std::process::exit(1);
                }
            }
            if let Some(mut o) = overlay {
                let _ = o.kill_and_wait();
            }
        }
        Commands::Hide => {
            if !cli.quiet {
                heading("Test: hide");
            }
            let timeout = cli.timeout;
            let logs = cli.logs;
            let mut overlay = None;
            if !cli.no_warn {
                overlay = crate::process::start_warn_overlay_with_delay();
            }
            match run_with_watchdog("hide", timeout, move || hide::run_hide_test(timeout, logs)) {
                Ok(()) => {
                    if !cli.quiet {
                        println!("hide: OK (toggle on/off roundtrip)")
                    }
                }
                Err(e) => {
                    eprintln!("hide: ERROR: {}", e);
                    print_hints(&e);
                    if let Some(mut o) = overlay {
                        let _ = o.kill_and_wait();
                    }
                    std::process::exit(1);
                }
            }
            if let Some(mut o) = overlay {
                let _ = o.kill_and_wait();
            }
        }
        Commands::FocusWinHelper { .. } => {
            // Already handled above
            unreachable!()
        }
        Commands::WarnOverlay => {
            // Already handled above
            unreachable!()
        }
        Commands::Ui => {
            if !cli.quiet {
                heading("Test: ui");
            }
            let timeout = cli.timeout;
            let mut overlay = None;
            if !cli.no_warn {
                overlay = crate::process::start_warn_overlay_with_delay();
            }
            match run_with_watchdog("ui", timeout, move || ui::run_ui_demo(timeout)) {
                Ok(sum) => {
                    if !cli.quiet {
                        println!(
                            "ui: OK (hud_seen={}, time_to_hud_ms={:?})",
                            sum.hud_seen, sum.time_to_hud_ms
                        );
                    }
                }
                Err(e) => {
                    eprintln!("ui: ERROR: {}", e);
                    print_hints(&e);
                    if let Some(mut o) = overlay {
                        let _ = o.kill_and_wait();
                    }
                    std::process::exit(1);
                }
            }
            if let Some(mut o) = overlay {
                let _ = o.kill_and_wait();
            }
        }
        // Screenshots extracted to separate tool: hotki-shots
        Commands::Minui => {
            if !cli.quiet {
                heading("Test: minui");
            }
            let timeout = cli.timeout;
            let mut overlay = None;
            if !cli.no_warn {
                overlay = crate::process::start_warn_overlay_with_delay();
            }
            match run_with_watchdog("minui", timeout, move || ui::run_minui_demo(timeout)) {
                Ok(sum) => {
                    if !cli.quiet {
                        println!(
                            "minui: OK (hud_seen={}, time_to_hud_ms={:?})",
                            sum.hud_seen, sum.time_to_hud_ms
                        );
                    }
                }
                Err(e) => {
                    eprintln!("minui: ERROR: {}", e);
                    print_hints(&e);
                    if let Some(mut o) = overlay {
                        let _ = o.kill_and_wait();
                    }
                    std::process::exit(1);
                }
            }
            if let Some(mut o) = overlay {
                let _ = o.kill_and_wait();
            }
        }
        Commands::Fullscreen { state, native } => {
            if !cli.quiet {
                heading("Test: fullscreen");
            }
            let toggle = match state {
                FsState::Toggle => Toggle::Toggle,
                FsState::On => Toggle::On,
                FsState::Off => Toggle::Off,
            };
            let timeout = cli.timeout;
            let logs = cli.logs;
            let mut overlay = None;
            if !cli.no_warn {
                overlay = crate::process::start_warn_overlay_with_delay();
            }
            match run_with_watchdog("fullscreen", timeout, move || {
                tests::fullscreen::run_fullscreen_test(timeout, logs, toggle, native)
            }) {
                Ok(()) => {
                    if !cli.quiet {
                        println!("fullscreen: OK (toggled non-native fullscreen)")
                    }
                }
                Err(e) => {
                    eprintln!("fullscreen: ERROR: {}", e);
                    print_hints(&e);
                    if let Some(mut o) = overlay {
                        let _ = o.kill_and_wait();
                    }
                    std::process::exit(1);
                }
            }
            if let Some(mut o) = overlay {
                let _ = o.kill_and_wait();
            }
        } // Preflight smoketest removed.
    }
}
