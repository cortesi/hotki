use std::{
    path::PathBuf,
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use clap::{Parser, Subcommand};
use tracing_subscriber::prelude::*;

#[derive(Parser, Debug)]
#[command(name = "smoketest", about = "Hotki smoketest tool", version)]
struct Cli {
    /// Enable logging to stdout/stderr at info level (respect RUST_LOG)
    #[arg(long)]
    logs: bool,
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Measure relay repeats posted to the focused window
    #[command(name = "repeat-relay")]
    Relay {
        /// Duration to hold in milliseconds
        #[arg(long, default_value_t = 2000)]
        time: u64,
    },
    /// Measure number of shell invocations when repeating a shell command
    #[command(name = "repeat-shell")]
    Shell {
        /// Duration to hold in milliseconds
        #[arg(long, default_value_t = 2000)]
        time: u64,
    },
    /// Measure repeats by incrementing system volume from zero
    #[command(name = "repeat-volume")]
    Volume {
        /// Duration to hold in milliseconds
        #[arg(long, default_value_t = 2000)]
        time: u64,
    },
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
        Commands::Relay { time } => repeat_relay(time),
        Commands::Shell { time } => repeat_shell(time),
        Commands::Volume { time } => repeat_volume(time),
        Commands::All => run_all_tests(),
        Commands::Ui => run_ui_demo(),
        Commands::Screenshots { theme, dir } => run_screenshots(theme, dir),
        Commands::Minui => run_minui_demo(),
    }
}

// Resolve the path to the hotki binary.
// Priority: $HOTKI_BIN env override -> sibling of current exe (target/{profile}/hotki)
fn resolve_hotki_bin() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("HOTKI_BIN") {
        let pb = PathBuf::from(p);
        if pb.exists() {
            return Some(pb);
        }
    }
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("hotki")))
        .filter(|p| p.exists())
}

// (intentionally left without a generic fullscreen capture; HUD-only capture below)

// Try to locate the NSWindow representing the HUD by matching the owner PID
// and the window title ("Hotki HUD"). Returns (window_id, bounds) on success.
fn find_hud_window(pid: u32) -> Option<(u32, (i32, i32, i32, i32))> {
    use core_foundation::dictionary::CFDictionaryRef;
    use core_foundation::{
        array::CFArray,
        base::{CFType, TCFType, TCFTypeRef},
        dictionary::CFDictionary,
        number::CFNumber,
        string::CFString,
    };
    use core_graphics2::window::{
        CGWindowListOption, copy_window_info, kCGNullWindowID, kCGWindowBounds, kCGWindowName,
        kCGWindowNumber, kCGWindowOwnerPID,
    };

    // Fetch on-screen windows
    let arr: CFArray = copy_window_info(CGWindowListOption::OnScreenOnly, kCGNullWindowID)?;
    // Iterate untyped array and wrap each entry as CFDictionary<CFStringRef, CFTypeRef>
    for item in arr.iter() {
        let dict_ref = unsafe { CFDictionaryRef::from_void_ptr(*item) };
        let dict: CFDictionary<CFString, CFType> =
            unsafe { CFDictionary::wrap_under_get_rule(dict_ref) };

        // Owner PID
        let owner_pid = unsafe { dict.find(kCGWindowOwnerPID) }
            .and_then(|v| v.downcast::<CFNumber>())
            .and_then(|n| n.to_i64().map(|v| v as u32))
            .unwrap_or_default();
        if owner_pid != pid {
            continue;
        }

        // Title
        let name = unsafe { dict.find(kCGWindowName) }
            .and_then(|v| v.downcast::<CFString>())
            .map(|s| s.to_string())
            .unwrap_or_default();
        if name != "Hotki HUD" {
            continue;
        }

        // Window ID
        let win_id: u32 = unsafe { dict.find(kCGWindowNumber) }
            .and_then(|v| v.downcast::<CFNumber>())
            .and_then(|n| n.to_i64().map(|v| v as u32))?;

        // Bounds
        let bdict_any = unsafe { dict.find(kCGWindowBounds) }?;
        let bdict_ref: CFDictionaryRef = bdict_any.as_CFTypeRef() as CFDictionaryRef;
        let bdict: CFDictionary<CFString, CFType> =
            unsafe { CFDictionary::wrap_under_get_rule(bdict_ref) };
        let kx = CFString::from_static_string("X");
        let ky = CFString::from_static_string("Y");
        let kw = CFString::from_static_string("Width");
        let kh = CFString::from_static_string("Height");
        let get = |k: &CFString| {
            bdict
                .find(k.clone())
                .and_then(|v| v.downcast::<CFNumber>())
                .and_then(|n| n.to_i64().map(|v| v as i32))
        };
        let (x, y, w, h) = (get(&kx)?, get(&ky)?, get(&kw)?, get(&kh)?);

        return Some((win_id, (x, y, w, h)));
    }
    None
}

// Capture just the HUD window by CGWindowID; fall back to rect if needed.
fn capture_hud_window(pid: u32, dir: &std::path::Path, name: &str) -> bool {
    use std::ffi::OsStr;
    if let Some((win_id, (x, y, w, h))) = find_hud_window(pid) {
        let sanitized = name
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                    c
                } else {
                    '_'
                }
            })
            .collect::<String>();
        let path = dir.join(format!("{}.png", sanitized));
        // Try window-id capture first (-l), without shadow (-o), no UI (-x)
        let status = Command::new("screencapture")
            .args([
                OsStr::new("-x"),
                OsStr::new("-o"),
                OsStr::new("-l"),
                std::ffi::OsString::from(win_id.to_string()).as_os_str(),
                path.as_os_str(),
            ])
            .status();
        if matches!(status, Ok(s) if s.success()) {
            return true;
        }
        // Fallback to rectangular region capture if -l fails
        let rect_arg = format!("{},{},{},{}", x, y, w, h);
        let status = Command::new("screencapture")
            .args([
                OsStr::new("-x"),
                OsStr::new("-R"),
                OsStr::new(&rect_arg),
                path.as_os_str(),
            ])
            .status();
        return matches!(status, Ok(s) if s.success());
    }
    false
}

// Find a visible notification window for a given PID (title="Hotki Notification").
fn find_notification_window(pid: u32) -> Option<(u32, (i32, i32, i32, i32))> {
    use core_foundation::dictionary::CFDictionaryRef;
    use core_foundation::{
        array::CFArray,
        base::{CFType, TCFType, TCFTypeRef},
        dictionary::CFDictionary,
        number::CFNumber,
        string::CFString,
    };
    use core_graphics2::window::{
        CGWindowListOption, copy_window_info, kCGNullWindowID, kCGWindowBounds, kCGWindowName,
        kCGWindowNumber, kCGWindowOwnerPID,
    };

    let arr: CFArray = copy_window_info(CGWindowListOption::OnScreenOnly, kCGNullWindowID)?;
    for item in arr.iter() {
        let dict_ref = unsafe { CFDictionaryRef::from_void_ptr(*item) };
        let dict: CFDictionary<CFString, CFType> =
            unsafe { CFDictionary::wrap_under_get_rule(dict_ref) };
        // Owner PID
        let owner_pid = unsafe { dict.find(kCGWindowOwnerPID) }
            .and_then(|v| v.downcast::<CFNumber>())
            .and_then(|n| n.to_i64().map(|v| v as u32))
            .unwrap_or_default();
        if owner_pid != pid {
            continue;
        }
        // Title
        let name = unsafe { dict.find(kCGWindowName) }
            .and_then(|v| v.downcast::<CFString>())
            .map(|s| s.to_string())
            .unwrap_or_default();
        if name != "Hotki Notification" {
            continue;
        }
        // Window ID
        let win_id: u32 = unsafe { dict.find(kCGWindowNumber) }
            .and_then(|v| v.downcast::<CFNumber>())
            .and_then(|n| n.to_i64().map(|v| v as u32))?;
        // Bounds
        let bdict_any = unsafe { dict.find(kCGWindowBounds) }?;
        let bdict_ref: CFDictionaryRef = bdict_any.as_CFTypeRef() as CFDictionaryRef;
        let bdict: CFDictionary<CFString, CFType> =
            unsafe { CFDictionary::wrap_under_get_rule(bdict_ref) };
        let kx = CFString::from_static_string("X");
        let ky = CFString::from_static_string("Y");
        let kw = CFString::from_static_string("Width");
        let kh = CFString::from_static_string("Height");
        let get = |k: &CFString| {
            bdict
                .find(k.clone())
                .and_then(|v| v.downcast::<CFNumber>())
                .and_then(|n| n.to_i64().map(|v| v as i32))
        };
        let (x, y, w, h) = (get(&kx)?, get(&ky)?, get(&kw)?, get(&kh)?);
        return Some((win_id, (x, y, w, h)));
    }
    None
}

fn capture_notification_window(pid: u32, dir: &std::path::Path, name: &str) -> bool {
    use std::ffi::OsStr;
    if let Some((win_id, (x, y, w, h))) = find_notification_window(pid) {
        let sanitized = name
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                    c
                } else {
                    '_'
                }
            })
            .collect::<String>();
        let path = dir.join(format!("{}.png", sanitized));
        // Try window-id capture first
        let status = Command::new("screencapture")
            .args([
                OsStr::new("-x"),
                OsStr::new("-o"),
                OsStr::new("-l"),
                std::ffi::OsString::from(win_id.to_string()).as_os_str(),
                path.as_os_str(),
            ])
            .status();
        if matches!(status, Ok(s) if s.success()) {
            return true;
        }
        // Fallback to rect
        let rect_arg = format!("{},{},{},{}", x, y, w, h);
        let status = Command::new("screencapture")
            .args([
                OsStr::new("-x"),
                OsStr::new("-R"),
                OsStr::new(&rect_arg),
                path.as_os_str(),
            ])
            .status();
        return matches!(status, Ok(s) if s.success());
    }
    false
}

// Build the hotki binary quietly if it's missing. Returns true if the binary
// exists afterwards. Build output is suppressed to avoid interleaved cargo logs.
fn ensure_hotki_built_quiet() -> bool {
    if resolve_hotki_bin().is_some() {
        return true;
    }
    let status = Command::new("cargo")
        .args(["build", "--bin", "hotki", "-q"]) // quiet: suppress progress
        .env("CARGO_TERM_COLOR", "never")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    if status.map(|s| s.success()).unwrap_or(false) {
        return resolve_hotki_bin().is_some();
    }
    false
}

// Relay activation chord once and wait for HUD depth>0 via UI IPC
fn ensure_hud_visible(sock: &str, timeout_ms: u64) -> bool {
    let rt = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("tokio runtime error: {}", e);
            return false;
        }
    };

    // Try to connect to the UI's server with retry until the global deadline.
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    let mut attempts = 0;
    let mut client = loop {
        match rt.block_on(async {
            hotki_server::Client::new_with_socket(sock)
                .with_connect_only()
                .connect()
                .await
        }) {
            Ok(c) => {
                println!("Connected to UI server after {} attempts", attempts + 1);
                break c;
            }
            Err(e) => {
                attempts += 1;
                if Instant::now() >= deadline {
                    eprintln!(
                        "Failed to connect to UI server after {} attempts: {}",
                        attempts, e
                    );
                    return false;
                }
                // Longer initial delay for first few attempts to give server time to start
                let delay = if attempts <= 3 { 200 } else { 50 };
                std::thread::sleep(Duration::from_millis(delay));
                continue;
            }
        }
    };
    // Borrow the inner connection once for the loop
    let conn = match client.connection() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Failed to get connection: {}", e);
            return false;
        }
    };

    // Fire the activation chord via HID so the tap sees it. If HUD doesn't
    // appear quickly, resend once per second until the deadline.
    let relayer = relaykey::RelayKey::new_unlabeled();
    let mut last_sent = None;
    if let Some(ch) = mac_keycode::Chord::parse("shift+cmd+0") {
        let pid = 0;
        relayer.key_down(pid, ch.clone(), false);
        std::thread::sleep(Duration::from_millis(80));
        relayer.key_up(pid, ch.clone());
        last_sent = Some(Instant::now());
    }

    // Wait for HudUpdate with HUD visible (viewing_root || path.len()>0) within timeout
    let mut seen_hud = false;
    while Instant::now() < deadline {
        let left = deadline.saturating_duration_since(Instant::now());
        // Poll in short chunks so we can resend activation if needed.
        let chunk = std::cmp::min(left, Duration::from_millis(300));
        let res = rt.block_on(async { tokio::time::timeout(chunk, conn.recv_event()).await });
        match res {
            Ok(Ok(msg)) => match msg {
                hotki_protocol::MsgToUI::HudUpdate { cursor, .. } => {
                    let depth = cursor.depth();
                    let visible = cursor.viewing_root || depth > 0;
                    println!("hud: depth={}, viewing_root={}", depth, cursor.viewing_root);
                    if visible {
                        seen_hud = true;
                        break;
                    }
                }
                other => println!("ui: {:?}", other),
            },
            Ok(Err(e)) => {
                eprintln!("recv_event error: {}", e);
                break;
            }
            // Timeout for this chunk; continue polling until global deadline
            Err(_) => {}
        }

        // If HUD hasn't appeared yet, occasionally resend the activation chord
        // to avoid waiting for a fixed pre-sleep.
        if let Some(last) = last_sent
            && last.elapsed() >= Duration::from_millis(1000)
        {
            if let Some(ch) = mac_keycode::Chord::parse("shift+cmd+0") {
                let pid = 0;
                relayer.key_down(pid, ch.clone(), false);
                std::thread::sleep(Duration::from_millis(80));
                relayer.key_up(pid, ch);
            }
            last_sent = Some(Instant::now());
        }
    }
    seen_hud
}

fn count_relay(ms: u64) -> usize {
    // Create a winit event loop; create the window from inside the loop (winit 0.30)
    let event_loop = winit::event_loop::EventLoop::new().unwrap();
    // window is managed within RelayApp

    // Tokio runtime for repeater ticker
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_time()
        .build()
        .expect("tokio runtime");
    let _guard = rt.enter();

    // Engine pieces
    let focus = hotki_engine::FocusHandler::new();
    let relay = hotki_engine::RelayHandler::new();
    let (tx, _rx) = hotki_protocol::ipc::ui_channel();
    let notifier = hotki_engine::NotificationDispatcher::new(tx);
    let repeater = hotki_engine::Repeater::new(focus.clone(), relay.clone(), notifier.clone());
    // Ensure repeat gating sees a valid pid
    focus.set_pid_for_tools(std::process::id() as i32);

    // Count repeats via observer hook
    struct Counter(AtomicUsize);
    impl hotki_engine::RepeatObserver for Counter {
        fn on_relay_repeat(&self, _id: &str) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }
    let counter = Arc::new(Counter(AtomicUsize::new(0)));
    repeater.set_repeat_observer(counter.clone());

    // Choose a chord that's safe to repeat
    let chord = mac_keycode::Chord::parse("right")
        .or_else(|| mac_keycode::Chord::parse("a"))
        .expect("parse chord");

    // Defer start until event loop begins so the window is visible
    let id = "smoketest-relay".to_string();
    // lifecycle state is managed within RelayApp

    // Drive a minimal event loop until time elapses, then stop (keyup)
    use winit::application::ApplicationHandler;
    use winit::event::WindowEvent;
    use winit::event_loop::{ActiveEventLoop, ControlFlow};
    struct RelayApp {
        repeater: hotki_engine::Repeater,
        window: Option<winit::window::Window>,
        id: String,
        chord: mac_keycode::Chord,
        started: bool,
        start: Option<Instant>,
        timeout: Duration,
    }

    impl ApplicationHandler for RelayApp {
        fn resumed(&mut self, elwt: &ActiveEventLoop) {
            if self.window.is_none() {
                let attrs = winit::window::Window::default_attributes()
                    .with_title("hotki smoketest: relayrepeat")
                    .with_visible(true);
                let win = elwt.create_window(attrs).expect("create window");
                if let Some(mtm) = objc2_foundation::MainThreadMarker::new() {
                    let app = objc2_app_kit::NSApplication::sharedApplication(mtm);
                    unsafe { app.activate() };
                }
                self.window = Some(win);
            }
        }

        fn window_event(
            &mut self,
            elwt: &ActiveEventLoop,
            _id: winit::window::WindowId,
            event: WindowEvent,
        ) {
            if let WindowEvent::CloseRequested = event {
                self.repeater.stop_sync(&self.id);
                elwt.exit();
            }
        }

        fn about_to_wait(&mut self, elwt: &ActiveEventLoop) {
            if !self.started {
                self.started = true;
                self.repeater.start_relay_repeat(
                    self.id.clone(),
                    self.chord.clone(),
                    Some(hotki_engine::RepeatSpec::default()),
                );
                self.start = Some(Instant::now());
            }
            if let Some(s) = self.start {
                if s.elapsed() >= self.timeout {
                    self.repeater.stop_sync(&self.id);
                    elwt.exit();
                }
                elwt.set_control_flow(ControlFlow::WaitUntil(s + self.timeout));
            } else {
                elwt.set_control_flow(ControlFlow::Wait);
            }
        }
    }

    let timeout = Duration::from_millis(ms);
    let mut app = RelayApp {
        repeater,
        window: None,
        id,
        chord,
        started: false,
        start: None,
        timeout,
    };
    let _ = event_loop.run_app(&mut app);

    counter.0.load(Ordering::SeqCst)
}

fn repeat_relay(ms: u64) {
    let n = count_relay(ms);
    println!("{} repeats", n);
}

fn sh_single_quote(s: &str) -> String {
    // POSIX single-quote escaping: 'foo' => '\'' inside single quotes
    let mut out = String::from("'");
    for ch in s.chars() {
        if ch == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(ch);
        }
    }
    out.push('\'');
    out
}

fn count_shell(ms: u64) -> usize {
    // Tokio runtime for repeater ticker
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_time()
        .build()
        .expect("tokio runtime");
    let _guard = rt.enter();

    // Engine pieces (no window required for shell execution)
    let focus = hotki_engine::FocusHandler::new();
    let relay = hotki_engine::RelayHandler::new();
    let (tx, _rx) = hotki_protocol::ipc::ui_channel();
    let notifier = hotki_engine::NotificationDispatcher::new(tx);
    let repeater = hotki_engine::Repeater::new(focus.clone(), relay.clone(), notifier.clone());

    // Create a unique temp file path and a tiny append command
    let path = std::env::temp_dir().join(format!(
        "hotki-smoketest-shell-{}-{}.log",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    // Ensure parent exists; create empty file
    let _ = std::fs::File::create(&path);
    let cmd = format!("printf . >> {}", sh_single_quote(&path.to_string_lossy()));

    // Start shell repeat (first run + ticker)
    let id = "smoketest-shell".to_string();
    repeater.start_shell_repeat(id.clone(), cmd, Some(hotki_engine::RepeatSpec::default()));

    // Wait for the specified duration
    std::thread::sleep(Duration::from_millis(ms));
    repeater.stop_sync(&id);

    // Read file and count bytes; subtract the initial run to get repeats
    let repeats = match std::fs::read(&path) {
        Ok(b) => b.len().saturating_sub(1),
        Err(_) => 0,
    };
    // Best-effort cleanup
    let _ = std::fs::remove_file(&path);
    repeats
}

fn repeat_shell(ms: u64) {
    let n = count_shell(ms);
    println!("{} repeats", n);
}

fn osascript(cmd: &str) -> std::io::Result<std::process::Output> {
    std::process::Command::new("osascript")
        .arg("-e")
        .arg(cmd)
        .output()
}

fn get_volume() -> Option<u64> {
    let out = osascript("output volume of (get volume settings)").ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout);
    s.trim().parse::<u64>().ok()
}

fn set_volume_abs(level: u8) -> bool {
    let cmd = format!("set volume output volume {}", level.min(100));
    osascript(&cmd).map(|o| o.status.success()).unwrap_or(false)
}

fn count_volume(ms: u64) -> usize {
    // Save current volume to restore later
    let original_volume = get_volume().unwrap_or(50);

    // Tokio runtime for repeater ticker
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_time()
        .build()
        .expect("tokio runtime");
    let _guard = rt.enter();

    // Reset to zero for testing
    let _ = set_volume_abs(0);

    // Build change_volume(1) command inline
    let script = "set currentVolume to output volume of (get volume settings)\nset volume output volume (currentVolume + 1)";
    let cmd = format!("osascript -e '{}'", script.replace('\n', "' -e '"));

    // Orchestrator for repeating shell
    let focus = hotki_engine::FocusHandler::new();
    let relay = hotki_engine::RelayHandler::new();
    let (tx, _rx) = hotki_protocol::ipc::ui_channel();
    let notifier = hotki_engine::NotificationDispatcher::new(tx);
    let repeater = hotki_engine::Repeater::new(focus.clone(), relay.clone(), notifier.clone());

    let id = "smoketest-volume".to_string();
    repeater.start_shell_repeat(id.clone(), cmd, Some(hotki_engine::RepeatSpec::default()));

    // Wait and stop
    std::thread::sleep(Duration::from_millis(ms));
    repeater.stop_sync(&id);

    // Measure resulting volume; subtract one for the initial run
    let vol = get_volume().unwrap_or(0);
    let repeats = vol.saturating_sub(1);

    // Restore original volume
    let _ = set_volume_abs(original_volume as u8);

    repeats as usize
}

fn repeat_volume(ms: u64) {
    // Save current volume to restore later
    let original_volume = get_volume().unwrap_or(50);

    let n = count_volume(ms);
    println!("{} repeats", n);

    // Ensure volume is restored even if count_volume doesn't complete normally
    let _ = set_volume_abs(original_volume as u8);
}

fn run_all_tests() {
    // Repeat tests: run for ~1s each (volume for 2s) and assert >= 3 repeats
    const MS: u64 = 1000;
    let relay = count_relay(MS);
    let shell = count_shell(MS);
    let volume = count_volume(2000);

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
    run_ui_demo();
    run_minui_demo();

    println!("All smoketests passed");
}

fn run_screenshots(theme: Option<String>, dir: PathBuf) {
    // Resolve paths
    let cwd = std::env::current_dir().expect("cwd");
    let cfg_path = cwd.join("examples/test.ron");
    if !cfg_path.exists() {
        eprintln!("Missing config: {}", cfg_path.display());
        return;
    }

    // Resolve the hotki binary path
    let Some(hotki_bin) = resolve_hotki_bin() else {
        eprintln!("Could not locate hotki binary. Set HOTKI_BIN or build it first.");
        return;
    };

    // If a theme override was requested, write a temp config with base_theme injected
    let used_cfg_path = if let Some(name) = theme.clone() {
        match std::fs::read_to_string(&cfg_path) {
            Ok(s) => {
                // If file already has base_theme, replace it; else, insert after opening '('
                let mut out = String::new();
                if s.contains("base_theme:") {
                    // Replace value between quotes after base_theme:
                    let re = regex::Regex::new("base_theme\\s*:\\s*\"[^\"]*\"").unwrap();
                    out = re
                        .replace(&s, format!("base_theme: \"{}\"", name))
                        .to_string();
                } else {
                    // Insert after first '('
                    if let Some(pos) = s.find('(') {
                        let (head, tail) = s.split_at(pos + 1);
                        out.push_str(head);
                        out.push('\n');
                        out.push_str(&format!("    base_theme: \"{}\",\n", name));
                        out.push_str(tail);
                    } else {
                        out = s; // fallback: shouldn't happen
                    }
                }
                let tmp = std::env::temp_dir().join(format!(
                    "hotki-smoketest-shots-{}-{}.ron",
                    std::process::id(),
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_nanos()
                ));
                if std::fs::write(&tmp, out).is_ok() {
                    tmp
                } else {
                    cfg_path.clone()
                }
            }
            Err(_) => cfg_path.clone(),
        }
    } else {
        cfg_path.clone()
    };

    // Launch server with the chosen config
    let mut hotki = std::process::Command::new(&hotki_bin)
        .env(
            "RUST_LOG",
            "info,hotki=info,hotki_server=info,hotki_engine=info,mac_hotkey=info,mac_focus_watcher=info,mrpc::connection=off",
        )
        .arg(&used_cfg_path)
        .spawn()
        .expect("launch hotki");

    let sock = hotki_server::socket_path_for_pid(hotki.id());

    // Wait for HUD visible
    let seen_hud = ensure_hud_visible(&sock, 6000);
    if !seen_hud {
        eprintln!("HUD did not appear");
    }

    // Ensure directory exists
    let _ = std::fs::create_dir_all(&dir);
    let pid = hotki.id();

    // Take HUD screenshot
    let _ = capture_hud_window(pid, &dir, "001_hud");

    // Enter Theme tester and trigger each notification type, capturing the window each time
    let gap = Duration::from_millis(160);
    let down_ms = Duration::from_millis(80);
    for (k, name) in [
        ("t", None), // enter Theme tester menu
        ("s", Some("002_notify_success")),
        ("i", Some("003_notify_info")),
        ("w", Some("004_notify_warning")),
        ("e", Some("005_notify_error")),
    ] {
        if let Some(ch) = mac_keycode::Chord::parse(k) {
            let relayer = relaykey::RelayKey::new_unlabeled();
            let pid0 = 0;
            relayer.key_down(pid0, ch.clone(), false);
            std::thread::sleep(down_ms);
            relayer.key_up(pid0, ch);
            std::thread::sleep(gap);
            if let Some(n) = name {
                // small settle time for notification animation
                std::thread::sleep(Duration::from_millis(120));
                let _ = capture_notification_window(pid, &dir, n);
            }
        }
    }

    // Exit HUD
    if let Some(ch) = mac_keycode::Chord::parse("shift+cmd+0") {
        let relayer = relaykey::RelayKey::new_unlabeled();
        relayer.key_down(0, ch.clone(), false);
        std::thread::sleep(down_ms);
        relayer.key_up(0, ch);
    }

    // Shutdown server via MRPC
    if let Ok(rt) = tokio::runtime::Runtime::new() {
        rt.block_on(async {
            if let Ok(mut c) = hotki_server::Client::new_with_socket(&sock)
                .with_connect_only()
                .connect()
                .await
            {
                let _ = c.shutdown_server().await;
            }
        });
    }

    let _ = hotki.kill();
    let _ = hotki.wait();
}

fn run_ui_demo() {
    // Resolve paths
    let cwd = std::env::current_dir().expect("cwd");
    let cfg_path = cwd.join("examples/test.ron");
    if !cfg_path.exists() {
        eprintln!("Missing config: {}", cfg_path.display());
        return;
    }

    // Resolve the hotki binary path
    let hotki_path = resolve_hotki_bin();
    let Some(hotki_bin) = hotki_path else {
        eprintln!("Could not locate hotki binary. Set HOTKI_BIN or build it first.");
        return;
    };

    // Launch hotki with the test config
    let mut hotki = std::process::Command::new(&hotki_bin)
        .env(
            "RUST_LOG",
            "info,hotki=info,hotki_server=info,hotki_engine=info,mac_hotkey=info,mac_focus_watcher=info,mrpc::connection=off",
        )
        .arg(cfg_path)
        .spawn()
        .expect("launch hotki");

    // Give the server more time to start up and create the socket
    std::thread::sleep(std::time::Duration::from_millis(2000));

    // Compute socket path and wait for HUD to appear
    let sock = hotki_server::socket_path_for_pid(hotki.id());
    let seen_hud = ensure_hud_visible(&sock, 10000);

    // Drive a short theme cycle if HUD appeared (screenshots already taken above)
    let mut seq: Vec<&str> = Vec::new();
    if seen_hud {
        // Enter Theme tester and cycle a bit
        seq.push("t");
        // Cycle to next theme 5 times
        seq.extend(std::iter::repeat_n("l", 5));
        seq.push("esc"); // back to main HUD
    }
    // Exit HUD
    seq.push("shift+cmd+0");
    let gap = Duration::from_millis(150);
    let down_ms = Duration::from_millis(80);
    for s in seq {
        if let Some(ch) = mac_keycode::Chord::parse(s) {
            // relay directly via untagged events so the tap sees them
            let relayer = relaykey::RelayKey::new_unlabeled();
            let pid = 0; // not used by RelayKey when posting to HID
            relayer.key_down(pid, ch.clone(), false);
            std::thread::sleep(down_ms);
            relayer.key_up(pid, ch);
            std::thread::sleep(gap);
        } else {
            eprintln!("failed to parse chord: {}", s);
            std::thread::sleep(gap);
        }
    }

    // Ask the server to shut down cleanly via MRPC (fresh connection)
    if let Ok(rt) = tokio::runtime::Runtime::new() {
        rt.block_on(async {
            if let Ok(mut c) = hotki_server::Client::new_with_socket(&sock)
                .with_connect_only()
                .connect()
                .await
            {
                let _ = c.shutdown_server().await;
            }
        });
    }

    // Best-effort shutdown and reap of hotki process
    let _ = hotki.kill();
    let _ = hotki.wait();
    if !seen_hud {
        eprintln!("HUD did not appear (no HudUpdate or depth change within 3s)");
        std::process::exit(1);
    }
}

fn run_minui_demo() {
    // Prepare a minimal config with mini HUD and a theme submenu
    let ron = r#"(
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
        style: (hud: (mode: mini)),
    )"#;

    // Write to a temp file
    let cfg_path = std::env::temp_dir().join(format!(
        "hotki-smoketest-minui-{}-{}.ron",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    if let Err(e) = std::fs::write(&cfg_path, ron) {
        eprintln!("Failed to write temp config: {}", e);
        return;
    }

    // Locate hotki binary
    let hotki_path = resolve_hotki_bin();
    let Some(hotki_bin) = hotki_path else {
        eprintln!("Could not locate hotki binary. Set HOTKI_BIN or build it first.");
        let _ = std::fs::remove_file(&cfg_path);
        return;
    };

    // Launch hotki with mini HUD config
    let mut hotki = std::process::Command::new(&hotki_bin)
        .arg(&cfg_path)
        .spawn()
        .expect("launch hotki");

    // Give the server more time to start up and create the socket
    std::thread::sleep(std::time::Duration::from_millis(2000));

    // Compute socket path and wait for HUD to appear
    let sock = hotki_server::socket_path_for_pid(hotki.id());
    let seen_hud = ensure_hud_visible(&sock, 10000);

    // Relay keys to drive mini HUD: activate, enter theme tester, cycle, back
    let mut seq: Vec<String> = Vec::new();
    if !seen_hud {
        eprintln!("HUD did not appear (no HudUpdate depth>0 within 3s)");
        let _ = hotki.kill();
        let _ = hotki.wait();
        let _ = std::fs::remove_file(&cfg_path);
        std::process::exit(1);
    }

    seq.push("t".to_string()); // enter theme tester (parent_title = "Theme tester")
    // Cycle to next theme 5 times
    seq.extend(std::iter::repeat_n("l".to_string(), 5));
    seq.push("esc".to_string()); // back
    seq.push("shift+cmd+0".to_string()); // exit

    let gap = std::time::Duration::from_millis(150);
    let down_ms = std::time::Duration::from_millis(80);
    for s in seq {
        if let Some(ch) = mac_keycode::Chord::parse(&s) {
            let relayer = relaykey::RelayKey::new_unlabeled();
            let pid = 0;
            relayer.key_down(pid, ch.clone(), false);
            std::thread::sleep(down_ms);
            relayer.key_up(pid, ch);
            std::thread::sleep(gap);
        } else {
            eprintln!("failed to parse chord: {}", s);
            std::thread::sleep(gap);
        }
    }

    // Ask the server to shut down via MRPC
    if let Ok(rt) = tokio::runtime::Runtime::new() {
        rt.block_on(async move {
            if let Ok(mut client) = hotki_server::Client::new_with_socket(&sock)
                .with_connect_only()
                .connect()
                .await
            {
                let _ = client.shutdown_server().await;
            }
        });
    }

    // Cleanup
    let _ = hotki.kill();
    let _ = hotki.wait();
    let _ = std::fs::remove_file(&cfg_path);
}
