use std::{
    path::PathBuf,
    process::{Command, Stdio},
};

use clap::{Parser, Subcommand};

mod config;
mod error;
mod focus;
mod hide;
mod raise;
mod repeat;
mod results;
mod screenshot;
mod session;
mod test_runner;
mod ui;
mod util;
mod winhelper;

use error::print_hints;
use tracing_subscriber::prelude::*;

#[derive(Parser, Debug)]
#[command(name = "smoketest", about = "Hotki smoketest tool", version)]
struct Cli {
    /// Enable logging to stdout/stderr at info level (respect RUST_LOG)
    #[arg(long)]
    logs: bool,

    /// Default duration for repeat tests in milliseconds
    #[arg(long, default_value_t = config::DEFAULT_DURATION_MS)]
    duration: u64,

    /// Default timeout for UI readiness and waits in milliseconds
    #[arg(long, default_value_t = config::DEFAULT_TIMEOUT_MS)]
    timeout: u64,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Measure relay repeats posted to the focused window
    #[command(name = "repeat-relay")]
    Relay {},
    /// Measure number of shell invocations when repeating a shell command
    #[command(name = "repeat-shell")]
    Shell {},
    /// Measure repeats by incrementing system volume from zero
    #[command(name = "repeat-volume")]
    Volume {},
    /// Run all smoketests (repeats + UI demos)
    #[command(name = "all")]
    All,
    /// Verify raise(action) by switching focus between two titled windows
    Raise,
    /// Verify focus tracking by activating a test window
    Focus,
    /// Verify hide(toggle)/on/off by moving a helper window off/on screen right
    Hide,
    /// Internal helper: create a foreground window with a title for focus testing
    #[command(hide = true, name = "focus-winhelper")]
    FocusWinHelper {
        /// Title to set on the helper window
        #[arg(long)]
        title: String,
        /// How long to keep the window alive (ms)
        #[arg(long, default_value_t = config::DEFAULT_HELPER_WINDOW_TIME_MS)]
        time: u64,
    },
    /// Launch UI with test config and drive a short HUD + theme cycle
    Ui,
    /// Take HUD-only screenshots for a theme
    #[command(name = "screenshots")]
    Screenshots {
        /// Theme name to apply before capturing (optional)
        #[arg(long)]
        theme: Option<String>,
        /// Output directory for PNG files
        dir: PathBuf,
    },
    /// Launch UI in mini HUD mode and cycle themes
    Minui,
    /// Check required permissions and screen capture capability
    Preflight,
}

// Re-export common result types
pub use results::{Summary, FocusOutcome, TestOutcome, TestDetails};

fn heading(title: &str) {
    println!("\n==> {}", title);
}

fn main() {
    let cli = Cli::parse();
    if cli.logs {
        // logs on: install a basic tracing subscriber and suppress mrpc disconnect noise
        let mut env_filter =
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into());
        if let Ok(d) = "mrpc::connection=off".parse() {
            env_filter = env_filter.add_directive(d);
        }
        let _ = tracing_subscriber::registry()
            .with(env_filter)
            .with(tracing_subscriber::fmt::layer().without_time())
            .try_init();
    }

    // Always build the hotki binary once at startup to avoid running against a stale build.
    heading("Building hotki");
    if !build_hotki_quiet() {
        eprintln!("Failed to build 'hotki' binary. Try: cargo build -p hotki");
        std::process::exit(1);
    }
    match cli.command {
        Commands::Relay { .. } => {
            heading("Test: repeat-relay");
            repeat_relay(cli.duration)
        }
        Commands::Shell { .. } => {
            heading("Test: repeat-shell");
            repeat_shell(cli.duration)
        }
        // Volume can be slightly slower; keep a floor to reduce flakiness
        Commands::Volume { .. } => {
            heading("Test: repeat-volume");
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
        Commands::FocusWinHelper { title, time } => {
            if let Err(e) = winhelper::run_focus_winhelper(&title, time) {
                eprintln!("focus-winhelper: ERROR: {}", e);
                std::process::exit(2);
            }
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

//

// (intentionally left without a generic fullscreen capture; HUD-only capture below)

// Try to locate the NSWindow representing the HUD by matching the owner PID
// and the window title ("Hotki HUD"). Returns (window_id, bounds) on success.
//

// Capture just the HUD window by CGWindowID; fall back to rect if needed.
//

// Find a visible notification window for a given PID (title="Hotki Notification").
//

//

// Build the hotki binary quietly (always). Returns true on success.
// Output is suppressed to avoid interleaved cargo logs.
fn build_hotki_quiet() -> bool {
    Command::new("cargo")
        .args(["build", "-q", "-p", "hotki"]) // build the package for the workspace
        .env("CARGO_TERM_COLOR", "never")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

//

use repeat::{count_relay, count_shell, count_volume, repeat_relay, repeat_shell, repeat_volume};

//

fn run_all_tests(duration_ms: u64, timeout_ms: u64) {
    // Repeat tests: run sequentially; print result immediately after each
    heading("Test: repeat-relay");
    let relay = count_relay(duration_ms);
    if relay < 3 {
        eprintln!("FAIL repeat-relay: {} repeats (< 3)", relay);
        tracing::error!("repeat-relay failed: {} repeats (< 3)", relay);
        std::process::exit(1);
    } else {
        println!("repeat-relay: {} repeats", relay);
    }

    heading("Test: repeat-shell");
    let shell = count_shell(duration_ms);
    if shell < 3 {
        eprintln!("FAIL repeat-shell: {} repeats (< 3)", shell);
        tracing::error!("repeat-shell failed: {} repeats (< 3)", shell);
        std::process::exit(1);
    } else {
        println!("repeat-shell: {} repeats", shell);
    }

    heading("Test: repeat-volume");
    let volume = count_volume(std::cmp::max(
        duration_ms,
        config::MIN_VOLUME_TEST_DURATION_MS,
    ));
    if volume < 3 {
        eprintln!("FAIL repeat-volume: {} repeats (< 3)", volume);
        tracing::error!("repeat-volume failed: {} repeats (< 3)", volume);
        std::process::exit(1);
    } else {
        println!("repeat-volume: {} repeats", volume);
    }

    // hotki was built once at startup; no additional build needed here.

    // Focus test: verify engine observes a frontmost window title change
    heading("Test: focus");
    match crate::focus::run_focus_test(timeout_ms, false) {
        Ok(out) => println!(
            "focus: OK (title='{}', pid={}, time_to_match_ms={})",
            out.title, out.pid, out.elapsed_ms
        ),
        Err(e) => {
            eprintln!("focus: ERROR: {}", e);
            tracing::error!("focus test failed: {}", e);
            print_hints(&e);
            std::process::exit(1);
        }
    }

    // Raise test: verify raise by title twice
    heading("Test: raise");
    match crate::raise::run_raise_test(timeout_ms, false) {
        Ok(()) => println!("raise: OK (raised by title twice)"),
        Err(e) => {
            eprintln!("raise: ERROR: {}", e);
            tracing::error!("raise test failed: {}", e);
            print_hints(&e);
            std::process::exit(1);
        }
    }

    // UI demos: ensure HUD appears and basic theme cycling works (ui + miniui)
    heading("Test: ui");
    match ui::run_ui_demo(timeout_ms) {
        Ok(s) => println!(
            "ui: OK (hud_seen={}, time_to_hud_ms={:?})",
            s.hud_seen, s.time_to_hud_ms
        ),
        Err(e) => {
            eprintln!("ui: ERROR: {}", e);
            tracing::error!("ui demo failed: {}", e);
            print_hints(&e);
            std::process::exit(1);
        }
    }
    heading("Test: minui");
    match ui::run_minui_demo(timeout_ms) {
        Ok(s) => println!(
            "minui: OK (hud_seen={}, time_to_hud_ms={:?})",
            s.hud_seen, s.time_to_hud_ms
        ),
        Err(e) => {
            eprintln!("minui: ERROR: {}", e);
            tracing::error!("minui demo failed: {}", e);
            print_hints(&e);
            std::process::exit(1);
        }
    }
    println!("All smoketests passed");
}

fn run_preflight() -> bool {
    // Accessibility and Input Monitoring via permissions crate
    let p = permissions::check_permissions();
    println!(
        "permissions: accessibility={}, input_monitoring={}",
        p.accessibility_ok, p.input_ok
    );

    // Screen Recording via screencapture
    use std::ffi::OsStr;
    let tmp = std::env::temp_dir().join(format!(
        "hotki-smoketest-preflight-{}-{}.png",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let status = Command::new("screencapture")
        .args([
            OsStr::new("-x"),
            OsStr::new("-R"),
            OsStr::new("0,0,1,1"),
            tmp.as_os_str(),
        ])
        .status();
    let mut screen_ok = false;
    if let Ok(st) = status {
        screen_ok = st.success();
    }
    let _ = std::fs::remove_file(&tmp);
    println!("screen_recording: {}", screen_ok);

    if !p.accessibility_ok {
        eprintln!(
            "hint: grant Accessibility permission to your terminal under System Settings → Privacy & Security → Accessibility"
        );
    }
    if !p.input_ok {
        eprintln!(
            "hint: grant Input Monitoring permission to your terminal under System Settings → Privacy & Security → Input Monitoring"
        );
    }
    if !screen_ok {
        eprintln!(
            "hint: grant Screen Recording permission under System Settings → Privacy & Security → Screen Recording"
        );
    }
    p.accessibility_ok && p.input_ok && screen_ok
}

//

//

//
