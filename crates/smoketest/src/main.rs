use std::{
    error::Error as StdError,
    fmt,
    path::PathBuf,
    process::{Command, Stdio},
};

use clap::{Parser, Subcommand};
mod repeat;
mod screenshot;
mod session;
mod ui;
mod util;
use tracing_subscriber::prelude::*;

#[derive(Parser, Debug)]
#[command(name = "smoketest", about = "Hotki smoketest tool", version)]
struct Cli {
    /// Enable logging to stdout/stderr at info level (respect RUST_LOG)
    #[arg(long)]
    logs: bool,

    /// Default duration for repeat tests in milliseconds
    #[arg(long, default_value_t = 1000)]
    duration: u64,

    /// Default timeout for UI readiness and waits in milliseconds
    #[arg(long, default_value_t = 10000)]
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

// Lightweight result types for UI/screenshot flows
#[derive(Debug, Clone)]
struct Summary {
    hud_seen: bool,
    time_to_hud_ms: Option<u64>,
}

impl Summary {
    fn new() -> Self {
        Self {
            hud_seen: false,
            time_to_hud_ms: None,
        }
    }
}

#[derive(Debug)]
enum SmkError {
    MissingConfig(PathBuf),
    HotkiBinNotFound,
    SpawnFailed(String),
    HudNotVisible { timeout_ms: u64 },
    CaptureFailed(&'static str),
    Io(std::io::Error),
}

impl fmt::Display for SmkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SmkError::MissingConfig(p) => write!(f, "missing config: {}", p.display()),
            SmkError::HotkiBinNotFound => write!(
                f,
                "could not locate 'hotki' binary (set HOTKI_BIN or `cargo build --bin hotki`)"
            ),
            SmkError::SpawnFailed(s) => write!(f, "failed to launch hotki: {}", s),
            SmkError::HudNotVisible { timeout_ms } => write!(
                f,
                "HUD did not appear within {} ms (no HudUpdate depth>0)",
                timeout_ms
            ),
            SmkError::CaptureFailed(which) => write!(f, "failed to capture {} window", which),
            SmkError::Io(e) => write!(f, "I/O error: {}", e),
        }
    }
}

impl StdError for SmkError {}

fn print_hints(err: &SmkError) {
    match err {
        SmkError::HotkiBinNotFound => {
            eprintln!("hint: set HOTKI_BIN to an existing binary or run: cargo build --bin hotki");
        }
        SmkError::HudNotVisible { .. } => {
            eprintln!("hint: the activation chord is sent via Accessibility (HID)");
            eprintln!(
                "      ensure the terminal/shell running smoketest is allowed under System Settings → Privacy & Security → Accessibility"
            );
            eprintln!("      also check hotki logs with --logs for server startup issues");
        }
        SmkError::CaptureFailed(_) => {
            eprintln!("hint: screencapture requires Screen Recording permission for the terminal");
            eprintln!(
                "      grant it under System Settings → Privacy & Security → Screen Recording"
            );
        }
        SmkError::MissingConfig(_) => {
            eprintln!(
                "hint: expected examples/test.ron relative to repo root (or pass a valid config)"
            );
        }
        SmkError::SpawnFailed(_) | SmkError::Io(_) => {}
    }
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
    match cli.command {
        Commands::Relay { .. } => repeat_relay(cli.duration),
        Commands::Shell { .. } => repeat_shell(cli.duration),
        // Volume can be slightly slower; keep a floor to reduce flakiness
        Commands::Volume { .. } => repeat_volume(std::cmp::max(cli.duration, 2000)),
        Commands::All => run_all_tests(cli.duration, cli.timeout),
        Commands::Ui => match ui::run_ui_demo(cli.timeout) {
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
        },
        Commands::Screenshots { theme, dir } => {
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
        Commands::Minui => match ui::run_minui_demo(cli.timeout) {
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
        },
        Commands::Preflight => {
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

// Build the hotki binary quietly if it's missing. Returns true if the binary
// exists afterwards. Build output is suppressed to avoid interleaved cargo logs.
fn ensure_hotki_built_quiet() -> bool {
    if util::resolve_hotki_bin().is_some() {
        return true;
    }
    let status = Command::new("cargo")
        .args(["build", "--bin", "hotki", "-q"]) // quiet: suppress progress
        .env("CARGO_TERM_COLOR", "never")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    if status.map(|s| s.success()).unwrap_or(false) {
        return util::resolve_hotki_bin().is_some();
    }
    false
}

//

use repeat::{count_relay, count_shell, count_volume, repeat_relay, repeat_shell, repeat_volume};

//

fn run_all_tests(duration_ms: u64, timeout_ms: u64) {
    // Repeat tests: use provided duration (with a floor for volume)
    let relay = count_relay(duration_ms);
    let shell = count_shell(duration_ms);
    let volume = count_volume(std::cmp::max(duration_ms, 2000));

    let mut ok = true;
    if relay < 3 {
        eprintln!("FAIL repeat-relay: {} repeats (< 3)", relay);
        ok = false;
    } else {
        println!("repeat-relay: {} repeats", relay);
    }
    if shell < 3 {
        eprintln!("FAIL repeat-shell: {} repeats (< 3)", shell);
        ok = false;
    } else {
        println!("repeat-shell: {} repeats", shell);
    }
    if volume < 3 {
        eprintln!("FAIL repeat-volume: {} repeats (< 3)", volume);
        ok = false;
    } else {
        println!("repeat-volume: {} repeats", volume);
    }

    if !ok {
        std::process::exit(1);
    }

    // Ensure the hotki app exists without interleaving cargo output.
    if !ensure_hotki_built_quiet() {
        eprintln!(
            "Could not locate or build 'hotki' binary. Set HOTKI_BIN or run 'cargo build --bin hotki' first."
        );
        std::process::exit(1);
    }

    // UI demos: ensure HUD appears and basic theme cycling works (ui + miniui)
    match ui::run_ui_demo(timeout_ms) {
        Ok(s) => println!(
            "ui: OK (hud_seen={}, time_to_hud_ms={:?})",
            s.hud_seen, s.time_to_hud_ms
        ),
        Err(e) => {
            eprintln!("ui: ERROR: {}", e);
            print_hints(&e);
            ok = false;
        }
    }
    match ui::run_minui_demo(timeout_ms) {
        Ok(s) => println!(
            "minui: OK (hud_seen={}, time_to_hud_ms={:?})",
            s.hud_seen, s.time_to_hud_ms
        ),
        Err(e) => {
            eprintln!("minui: ERROR: {}", e);
            print_hints(&e);
            ok = false;
        }
    }

    if !ok {
        std::process::exit(1);
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
