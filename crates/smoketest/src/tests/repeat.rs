//! Repeat throughput smoketests.
//!
//! What this verifies
//! - These routines measure repeat throughput for three paths and print a
//!   count after running for a caller-supplied duration:
//!   - `repeat_relay`: Post repeated key events to the focused window using the
//!     engine’s relay path and count repeat callbacks.
//!   - `repeat_shell`: Execute a tiny shell command repeatedly and count file
//!     bytes written.
//!   - `repeat_volume`: Repeatedly bump system output volume from zero and
//!     derive the count from the final volume.
//!
//! Acceptance criteria
//! - Each routine runs for approximately the requested duration and completes
//!   without panic, printing a non-negative repeat count to stdout.
//! - No explicit minimum throughput is asserted; failures are defined as
//!   runtime errors (e.g., inability to create/read files or interact with the
//!   system volume), not low counts.
//! - `repeat_volume` restores the original system volume on exit.
use std::{
    option::Option,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use winit::event_loop::EventLoop;

use crate::{config, process};

pub fn count_relay(ms: u64) -> usize {
    let event_loop = EventLoop::new().unwrap();

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_time()
        .build()
        .expect("tokio runtime");
    let _guard = rt.enter();

    let focus_ctx = Arc::new(Mutex::new(None::<(String, String, i32)>));
    let relay = hotki_engine::RelayHandler::new();
    let (tx, _rx) = hotki_protocol::ipc::ui_channel();
    let notifier = hotki_engine::NotificationDispatcher::new(tx);
    let repeater =
        hotki_engine::Repeater::new_with_ctx(focus_ctx.clone(), relay.clone(), notifier.clone());
    if let Ok(mut f) = focus_ctx.lock() {
        *f = Some((
            "smoketest-app".to_string(),
            crate::config::RELAY_TEST_TITLE.to_string(),
            std::process::id() as i32,
        ));
    }

    struct Counter(AtomicUsize);
    impl hotki_engine::RepeatObserver for Counter {
        fn on_relay_repeat(&self, _id: &str) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }
    let counter = Arc::new(Counter(AtomicUsize::new(0)));
    repeater.set_repeat_observer(counter.clone());

    let chord = mac_keycode::Chord::parse("right")
        .or_else(|| mac_keycode::Chord::parse("a"))
        .expect("parse chord");

    use winit::{
        application::ApplicationHandler,
        event::WindowEvent,
        event_loop::{ActiveEventLoop, ControlFlow},
    };
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
                use winit::dpi::{LogicalPosition, LogicalSize};
                let attrs = winit::window::Window::default_attributes()
                    .with_title(config::RELAY_TEST_TITLE)
                    .with_visible(true)
                    // Make the popup smaller to reduce intrusion.
                    .with_inner_size(LogicalSize::new(
                        crate::config::HELPER_WIN_WIDTH,
                        crate::config::HELPER_WIN_HEIGHT,
                    ));
                let win = elwt.create_window(attrs).expect("create window");
                if let Some(mtm) = objc2_foundation::MainThreadMarker::new() {
                    let app = objc2_app_kit::NSApplication::sharedApplication(mtm);
                    unsafe { app.activate() };
                }
                // Place the window at the top-right of the main screen.
                if let Some(mtm) = objc2_foundation::MainThreadMarker::new() {
                    use objc2_app_kit::NSScreen;
                    let margin: f64 = crate::config::HELPER_WIN_MARGIN;
                    if let Some(scr) = NSScreen::mainScreen(mtm) {
                        let vf = scr.visibleFrame();
                        let w = crate::config::HELPER_WIN_WIDTH;
                        let x = (vf.origin.x + vf.size.width - w - margin).max(0.0);
                        // Use small Y from the visible frame's origin for top anchoring
                        let y = (vf.origin.y + margin).max(0.0);
                        win.set_outer_position(LogicalPosition::new(x, y));
                    }
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
    let timeout = config::ms(ms);
    let mut app = RelayApp {
        repeater,
        window: None,
        id: "smoketest-relay".into(),
        chord,
        started: false,
        start: None,
        timeout,
    };
    let _ = event_loop.run_app(&mut app);
    counter.0.load(Ordering::SeqCst)
}

pub fn repeat_relay(ms: u64) {
    println!("{} repeats", count_relay(ms));
}

fn sh_single_quote(s: &str) -> String {
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

pub fn count_shell(ms: u64) -> usize {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_time()
        .build()
        .expect("tokio runtime");
    let _guard = rt.enter();

    let focus_ctx = Arc::new(Mutex::new(None::<(String, String, i32)>));
    let relay = hotki_engine::RelayHandler::new();
    let (tx, _rx) = hotki_protocol::ipc::ui_channel();
    let notifier = hotki_engine::NotificationDispatcher::new(tx);
    let repeater =
        hotki_engine::Repeater::new_with_ctx(focus_ctx.clone(), relay.clone(), notifier.clone());

    let path = std::env::temp_dir().join(format!(
        "hotki-smoketest-shell-{}-{}.log",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::File::create(&path);
    let cmd = format!("printf . >> {}", sh_single_quote(&path.to_string_lossy()));

    let id = "smoketest-shell".to_string();
    repeater.start_shell_repeat(id.clone(), cmd, Some(hotki_engine::RepeatSpec::default()));
    std::thread::sleep(config::ms(ms));
    repeater.stop_sync(&id);

    let repeats = match std::fs::read(&path) {
        Ok(b) => b.len().saturating_sub(1),
        Err(_) => 0,
    };
    let _ = std::fs::remove_file(&path);
    repeats
}

pub fn repeat_shell(ms: u64) {
    println!("{} repeats", count_shell(ms));
}

fn get_volume() -> Option<u64> {
    let out = process::osascript("output volume of (get volume settings)").ok()?;
    out.trim().parse::<u64>().ok()
}

fn set_volume_abs(level: u8) -> bool {
    let cmd = format!("set volume output volume {}", level.min(100));
    process::osascript(&cmd).is_ok()
}

pub fn count_volume(ms: u64) -> usize {
    let original_volume = get_volume().unwrap_or(50);
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_time()
        .build()
        .expect("tokio runtime");
    let _guard = rt.enter();
    let _ = set_volume_abs(0);

    let script = "set currentVolume to output volume of (get volume settings)\nset volume output volume (currentVolume + 1)";
    let cmd = format!("osascript -e '{}'", script.replace('\n', "' -e '"));

    let focus_ctx = Arc::new(Mutex::new(None::<(String, String, i32)>));
    let relay = hotki_engine::RelayHandler::new();
    let (tx, _rx) = hotki_protocol::ipc::ui_channel();
    let notifier = hotki_engine::NotificationDispatcher::new(tx);
    let repeater =
        hotki_engine::Repeater::new_with_ctx(focus_ctx.clone(), relay.clone(), notifier.clone());

    let id = "smoketest-volume".to_string();
    repeater.start_shell_repeat(id.clone(), cmd, Some(hotki_engine::RepeatSpec::default()));
    std::thread::sleep(config::ms(ms));
    repeater.stop_sync(&id);

    let vol = get_volume().unwrap_or(0);
    let repeats = vol.saturating_sub(1);
    let _ = set_volume_abs(original_volume as u8);
    repeats as usize
}

pub fn repeat_volume(ms: u64) {
    let n = count_volume(ms);
    println!("{} repeats", n);
}
