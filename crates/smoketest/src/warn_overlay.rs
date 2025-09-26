use std::{
    env, fs,
    path::PathBuf,
    process::{self as std_process, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use objc2::rc::{Retained, autoreleasepool};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};
use winit::{
    application::ApplicationHandler,
    dpi::{LogicalPosition, LogicalSize},
    event::WindowEvent,
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
    window::{Window, WindowId, WindowLevel},
};

use crate::{
    config,
    error::Result as SmoketestResult,
    process::{self as process_utils, ManagedChild},
};

/// Internal flag passed to the smoketest binary to start the warn overlay helper.
pub const WARN_OVERLAY_STANDALONE_FLAG: &str = "--hotki-internal-warn-overlay";

/// Temp-file path that carries the active smoketest status message.
pub fn status_file_path() -> PathBuf {
    overlay_path_for_current_run("status")
}

/// Temp-file path that carries auxiliary overlay information.
pub fn info_file_path() -> PathBuf {
    overlay_path_for_current_run("info")
}

/// Best-effort helper that writes `text` into the overlay’s shared temp files.
pub fn write_overlay_text(label: &str, text: &str) {
    let path = overlay_path_for_current_run(label);
    if let Err(_err) = fs::write(path, text.as_bytes()) {}
}

/// Compose the per-run overlay temp-file path for the given label.
fn overlay_path_for_current_run(label: &str) -> PathBuf {
    env::temp_dir().join(format!("hotki-smoketest-{label}-{}.txt", std_process::id()))
}

/// Spawn the warn overlay helper process and return a managed child handle.
fn spawn_overlay_child() -> SmoketestResult<ManagedChild> {
    let exe = env::current_exe()?;
    let status_path = status_file_path();
    let info_path = info_file_path();
    let mut cmd = Command::new(exe);
    cmd.env("HOTKI_SKIP_BUILD", "1")
        .arg(WARN_OVERLAY_STANDALONE_FLAG)
        .arg("--status-path")
        .arg(status_path)
        .arg("--info-path")
        .arg(info_path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let child = process_utils::spawn_managed(cmd)?;
    Ok(child)
}

/// Long-lived overlay instance used for multi-case smoketest runs.
pub struct OverlaySession {
    /// Child process hosting the hands-off overlay window.
    child: ManagedChild,
}

impl OverlaySession {
    /// Start the overlay and wait for the initial countdown to complete.
    pub fn start() -> Option<Self> {
        let status_path = status_file_path();
        if fs::write(&status_path, b"").is_err() {
            // best effort, ignore failure
        }
        let info_path = info_file_path();
        if fs::write(&info_path, b"").is_err() {
            // best effort, ignore failure
        }
        match spawn_overlay_child() {
            Ok(child) => {
                thread::sleep(Duration::from_millis(config::WARN_OVERLAY.initial_delay_ms));
                Some(Self { child })
            }
            Err(_) => None,
        }
    }

    /// Update the status line shown in the overlay window. Best-effort.
    pub fn set_status(&self, name: &str) {
        write_overlay_text("status", name);
    }

    /// Update the auxiliary info line in the overlay window. Best-effort.
    pub fn set_info(&self, info: &str) {
        write_overlay_text("info", info);
    }
}

impl Drop for OverlaySession {
    fn drop(&mut self) {
        if let Err(e) = self.child.kill_and_wait() {
            eprintln!("overlay: failed to stop helper: {}", e);
        }
    }
}

/// Internal app container for the warn overlay window and labels.
struct OverlayApp {
    /// Handle to the overlay window, if created.
    window: Option<Window>,
    /// Label that displays the current test title.
    title_label: Option<Retained<objc2_app_kit::NSTextField>>,
    /// Label that displays a warning message.
    warn_label: Option<Retained<objc2_app_kit::NSTextField>>,
    /// Label that displays auxiliary information.
    info_label: Option<Retained<objc2_app_kit::NSTextField>>,
    /// Label that displays the countdown/spinner.
    countdown_label: Option<Retained<objc2_app_kit::NSTextField>>,
    /// Optional path to the status file with the current test name.
    status_path: Option<PathBuf>,
    /// Optional path to a file with additional info text.
    info_path: Option<PathBuf>,
    /// Last title we displayed to avoid redundant updates.
    last_title: String,
    /// Last info text we displayed to avoid redundant updates.
    last_info: String,
    /// Next time we should refresh UI state.
    next_deadline: Option<Instant>,
    /// Start time used to drive countdown/spinner animations.
    start_time: Instant,
    /// Whether the countdown animation is currently active.
    countdown_active: bool,
    /// Fatal error captured during setup; returned after run loop exits.
    error: Option<String>,
}

impl OverlayApp {
    /// Attach overlay labels to the new window and configure fonts/positions.
    fn attach_labels(
        &mut self,
        mtm: objc2_foundation::MainThreadMarker,
        view: &objc2_app_kit::NSView,
        _vf: objc2_foundation::NSRect,
    ) {
        use objc2_app_kit::{NSColor, NSFont, NSTextAlignment, NSTextField};
        let margin_x: f64 = 12.0;
        let lw = (config::WARN_OVERLAY.width_px - 2.0 * margin_x).max(10.0);

        // Title label
        let title_text = NSString::from_str("...");
        let title = unsafe { NSTextField::labelWithString(&title_text, mtm) };
        let title_font = unsafe { NSFont::boldSystemFontOfSize(24.0) };
        unsafe { title.setFont(Some(&title_font)) };
        unsafe { title.setAlignment(NSTextAlignment::Center) };
        let title_h: f64 = 32.0;
        let title_y = (config::WARN_OVERLAY.height_px - title_h) / 2.0;
        let title_frame = NSRect::new(NSPoint::new(margin_x, title_y), NSSize::new(lw, title_h));
        unsafe { title.setFrame(title_frame) };
        unsafe { view.addSubview(&title) };

        // Countdown/spinner label
        let countdown_text = NSString::from_str("2");
        let countdown = unsafe { NSTextField::labelWithString(&countdown_text, mtm) };
        let countdown_font = unsafe { NSFont::boldSystemFontOfSize(32.0) };
        unsafe { countdown.setFont(Some(&countdown_font)) };
        unsafe { countdown.setAlignment(NSTextAlignment::Center) };
        let countdown_color =
            unsafe { NSColor::colorWithCalibratedRed_green_blue_alpha(0.2, 0.6, 1.0, 1.0) };
        unsafe { countdown.setTextColor(Some(&countdown_color)) };
        let countdown_h: f64 = 40.0;
        let countdown_y = title_y + title_h + 4.0;
        let countdown_frame = NSRect::new(
            NSPoint::new(margin_x, countdown_y),
            NSSize::new(lw, countdown_h),
        );
        unsafe { countdown.setFrame(countdown_frame) };
        unsafe { view.addSubview(&countdown) };

        // Info label
        let info_text = NSString::from_str("");
        let info = unsafe { NSTextField::labelWithString(&info_text, mtm) };
        let info_font = unsafe { NSFont::systemFontOfSize(13.0) };
        unsafe { info.setFont(Some(&info_font)) };
        unsafe { info.setAlignment(NSTextAlignment::Center) };
        let info_color = unsafe { NSColor::secondaryLabelColor() };
        unsafe { info.setTextColor(Some(&info_color)) };
        let info_h: f64 = 18.0;
        let info_y = (title_y - info_h - 4.0).max(24.0);
        let info_frame = NSRect::new(NSPoint::new(margin_x, info_y), NSSize::new(lw, info_h));
        unsafe { info.setFrame(info_frame) };
        unsafe { view.addSubview(&info) };

        // Warning text
        let warn_text = NSString::from_str("Hands off keyboard during tests");
        let warn = unsafe { NSTextField::labelWithString(&warn_text, mtm) };
        let warn_font = unsafe { NSFont::systemFontOfSize(12.0) };
        unsafe { warn.setFont(Some(&warn_font)) };
        unsafe { warn.setAlignment(NSTextAlignment::Center) };
        let warn_color = unsafe { NSColor::secondaryLabelColor() };
        unsafe { warn.setTextColor(Some(&warn_color)) };
        let warn_h: f64 = 16.0;
        let warn_y = 8.0;
        let warn_frame = NSRect::new(NSPoint::new(margin_x, warn_y), NSSize::new(lw, warn_h));
        unsafe { warn.setFrame(warn_frame) };
        unsafe { view.addSubview(&warn) };

        // Stash refs
        self.warn_label = Some(warn);
        self.title_label = Some(title);
        self.info_label = Some(info);
        self.countdown_label = Some(countdown);
    }
}

impl ApplicationHandler for OverlayApp {
    fn resumed(&mut self, elwt: &ActiveEventLoop) {
        if self.window.is_none() {
            let attrs = Window::default_attributes()
                .with_title("hotki smoketest: hands-off")
                .with_visible(true)
                .with_decorations(false)
                .with_resizable(false)
                .with_inner_size(LogicalSize::new(
                    config::WARN_OVERLAY.width_px,
                    config::WARN_OVERLAY.height_px,
                ));
            let win = match elwt.create_window(attrs) {
                Ok(w) => w,
                Err(e) => {
                    self.error = Some(format!("warn_overlay: failed to create window: {}", e));
                    elwt.exit();
                    return;
                }
            };
            // Ensure always-on-top using runtime API if available
            #[allow(unused_must_use)]
            {
                // Not all platforms/versions support this; ignore if missing at runtime.
                win.set_window_level(WindowLevel::AlwaysOnTop);
            }

            // Place centered on the main screen without activating the app
            if let Some(mtm) = objc2_foundation::MainThreadMarker::new() {
                // Reduce activation prominence to avoid focus changes.
                let app = objc2_app_kit::NSApplication::sharedApplication(mtm);
                app.setActivationPolicy(objc2_app_kit::NSApplicationActivationPolicy::Accessory);
                if let Some(scr) = objc2_app_kit::NSScreen::mainScreen(mtm) {
                    let vf = scr.visibleFrame();
                    let x = vf.origin.x + (vf.size.width - config::WARN_OVERLAY.width_px) / 2.0;
                    let y = vf.origin.y + (vf.size.height - config::WARN_OVERLAY.height_px) / 2.0;
                    win.set_outer_position(LogicalPosition::new(x.max(0.0), y.max(0.0)));

                    // Attach overlay labels to the window's content view
                    let windows = app.windows();
                    for w in windows.iter() {
                        let title = w.title();
                        let is_match = autoreleasepool(|pool| unsafe {
                            title.to_str(pool) == "hotki smoketest: hands-off"
                        });
                        if !is_match {
                            continue;
                        }
                        if let Some(view) = w.contentView() {
                            self.attach_labels(mtm, &view, vf);
                        }
                        break;
                    }
                }
            }

            self.window = Some(win);

            // Status path is pre-configured before run loop starts.
        }
    }

    fn window_event(&mut self, elwt: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        if let WindowEvent::CloseRequested = event {
            // Allow closing if user insists; otherwise parent will kill us when done.
            elwt.exit();
        }
    }

    fn about_to_wait(&mut self, elwt: &ActiveEventLoop) {
        // Periodically poll the status file and update title if it changed
        let now = Instant::now();

        // Check if we should update (either no deadline set yet, or deadline passed)
        let should_update = self.next_deadline.is_none_or(|deadline| now >= deadline);

        if should_update {
            // Update countdown timer or spinner
            if self.countdown_active {
                let elapsed = now.duration_since(self.start_time);
                let grace_ms = config::WARN_OVERLAY.initial_delay_ms;
                let remaining_ms = grace_ms.saturating_sub(elapsed.as_millis() as u64);

                if let Some(countdown_label) = &self.countdown_label
                    && let Some(_mtm) = objc2_foundation::MainThreadMarker::new()
                {
                    if remaining_ms > 0 {
                        // Show countdown number
                        let secs = remaining_ms.div_ceil(1000); // Round up
                        let text = format!("{}", secs);
                        let ns = NSString::from_str(&text);
                        unsafe { countdown_label.setStringValue(&ns) };
                    } else {
                        // Show spinner animation
                        let spinner_frames = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
                        let frame_idx =
                            ((elapsed.as_millis() / 100) as usize) % spinner_frames.len();
                        let ns = NSString::from_str(spinner_frames[frame_idx]);
                        unsafe { countdown_label.setStringValue(&ns) };
                    }
                    if let Some(w) = &self.window {
                        w.request_redraw();
                    }
                }
            }

            // Update test name from status file
            if let (Some(path), Some(label)) = (&self.status_path, &self.title_label)
                && let Ok(s) = fs::read_to_string(path)
            {
                let name = s.trim();
                if !name.is_empty()
                    && name != self.last_title
                    && let Some(_mtm) = objc2_foundation::MainThreadMarker::new()
                {
                    // Check if transitioning from prep to a real test
                    if (self.last_title == "..." || self.last_title == "Preparing tests...")
                        && name != "Preparing tests..."
                    {
                        // Real test starting, but keep animating spinner
                        // self.countdown_active = false;
                    }

                    // Show the test name, or "..." if still preparing
                    let display_name = if name == "Preparing tests..." {
                        "..."
                    } else {
                        name
                    };

                    let ns = NSString::from_str(display_name);
                    unsafe { label.setStringValue(&ns) };
                    self.last_title = name.to_string();
                    if let Some(w) = &self.window {
                        w.request_redraw();
                    }
                }
            }

            // Update info text from info file
            if let (Some(path), Some(label)) = (&self.info_path, &self.info_label)
                && let Ok(s) = fs::read_to_string(path)
            {
                let text = s.trim();
                if let Some(_mtm) = objc2_foundation::MainThreadMarker::new() {
                    let display = if text.is_empty() { " " } else { text };
                    let ns = NSString::from_str(display);
                    unsafe { label.setStringValue(&ns) };
                    self.last_info = display.to_string();
                    if let Some(w) = &self.window {
                        w.request_redraw();
                    }
                }
            }
            self.next_deadline = Some(now + Duration::from_millis(100));
        }
        let next = self
            .next_deadline
            .unwrap_or_else(|| now + Duration::from_millis(100));
        elwt.set_control_flow(ControlFlow::WaitUntil(next));
    }
}

/// Borderless, always-on-top overlay window instructing the user to avoid typing.
/// The window stays up until the process is killed by the parent orchestrator.
pub fn run_warn_overlay(
    status_path_arg: Option<PathBuf>,
    info_path_arg: Option<PathBuf>,
) -> Result<(), String> {
    // Create winit event loop; do not explicitly activate the app to avoid stealing focus.
    let event_loop = EventLoop::new().map_err(|e| e.to_string())?;
    let mut app = OverlayApp {
        window: None,
        title_label: None,
        warn_label: None,
        info_label: None,
        countdown_label: None,
        status_path: status_path_arg,
        info_path: info_path_arg,
        last_title: String::from("..."),
        last_info: String::new(),
        next_deadline: None,
        start_time: Instant::now(),
        countdown_active: true,
        error: None,
    };
    event_loop.run_app(&mut app).map_err(|e| e.to_string())?;
    if let Some(e) = app.error.take() {
        Err(e)
    } else {
        Ok(())
    }
}
