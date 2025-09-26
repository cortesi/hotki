use std::{
    process::id,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use hotki_world_ids::WorldWindowId;
use mac_winops::{self, AxProps, Rect, WindowInfo, screen};
use objc2::rc::autoreleasepool;
use objc2_app_kit::NSWindow;
use tracing::debug;
use winit::{
    application::ApplicationHandler,
    dpi::{LogicalPosition, LogicalSize, PhysicalPosition, PhysicalSize},
    event::WindowEvent,
    event_loop::{ActiveEventLoop, ControlFlow},
    window::{Window, WindowId},
};

use super::{
    config,
    scenario::{HelperConfig, Quirk, format_quirks},
    world,
};
use crate::{MinimizedPolicy, PlaceAttemptOptions, PlaceOptions, RaiseStrategy};

type TargetRect = ((f64, f64), (f64, f64), &'static str);

fn should_skip_apply_for_minimized(quirks: &[Quirk], minimized: bool) -> bool {
    minimized
        && quirks
            .iter()
            .any(|q| matches!(q, Quirk::IgnoreMoveIfMinimized))
}

fn select_sibling_for_cycle<'a>(
    windows: &'a [WindowInfo],
    pid: i32,
    current_title: &str,
    slug_fragment: &str,
) -> Option<&'a WindowInfo> {
    windows
        .iter()
        .find(|w| w.pid == pid && w.title != current_title && w.title.contains(slug_fragment))
}

fn parse_decorated_label(title: &str) -> Option<&str> {
    let start = title.rfind('[')?;
    let end = title.rfind(']')?;
    if end <= start {
        return None;
    }
    Some(&title[start + 2..end])
}

/// Parameter bundle for constructing a [`HelperApp`].
pub(super) struct HelperParams {
    /// Window title shown on the helper surface.
    pub(super) title: String,
    /// Scenario slug used for diagnostics.
    pub(super) scenario_slug: Arc<str>,
    /// Window label tied to artifacts and diagnostics.
    pub(super) window_label: Arc<str>,
    /// Total runtime for the helper window before forced shutdown.
    pub(super) time_ms: u64,
    /// Delay before applying position updates when directly setting frames.
    pub(super) delay_setframe_ms: u64,
    /// Delay before invoking the primary placement operation.
    pub(super) delay_apply_ms: u64,
    /// Duration for tweened placement animations.
    pub(super) tween_ms: u64,
    /// Absolute placement target rectangle, when provided.
    pub(super) apply_target: Option<(f64, f64, f64, f64)>,
    /// Grid placement specification, when provided.
    pub(super) apply_grid: Option<(u32, u32, u32, u32)>,
    /// Optional slot identifier for 2x2 layouts.
    pub(super) slot: Option<u8>,
    /// Optional grid dimensions and coordinates.
    pub(super) grid: Option<(u32, u32, u32, u32)>,
    /// Optional explicit window size.
    pub(super) size: Option<(f64, f64)>,
    /// Optional explicit window position.
    pub(super) pos: Option<(f64, f64)>,
    /// Optional overlay label text.
    pub(super) label_text: Option<String>,
    /// Optional minimum content size.
    pub(super) min_size: Option<(f64, f64)>,
    /// Optional rounding step for requested sizes.
    pub(super) step_size: Option<(f64, f64)>,
    /// Whether the helper launches minimized.
    pub(super) start_minimized: bool,
    /// Whether the helper launches zoomed (macOS zoom behavior).
    pub(super) start_zoomed: bool,
    /// Whether the helper window should be non-movable.
    pub(super) panel_nonmovable: bool,
    /// Whether the helper window should be non-resizable.
    pub(super) panel_nonresizable: bool,
    /// Whether to attach a modal sheet on launch.
    pub(super) attach_sheet: bool,
    /// Quirk list influencing runtime behaviour.
    pub(super) quirks: Vec<Quirk>,
    /// Placement strategy applied to this helper.
    pub(super) place: PlaceOptions,
    /// Shutdown flag shared with external callers.
    pub(super) shutdown: Arc<AtomicBool>,
}

/// State machine orchestrating the smoketest helper window lifecycle.
pub(super) struct HelperApp {
    /// Handle to the helper window, if created.
    window: Option<Window>,
    /// Window title used to locate the NSWindow for tweaks.
    title: String,
    /// Scenario slug for diagnostics.
    scenario_slug: Arc<str>,
    /// Helper label for diagnostics.
    window_label: Arc<str>,
    /// Time at which the helper should terminate.
    deadline: Instant,
    /// Time at which post-creation setup may run.
    post_create_ready_at: Option<Instant>,
    /// Delay before applying a set_frame operation (ms).
    delay_setframe_ms: u64,
    /// Delay before applying the main placement (ms).
    delay_apply_ms: u64,
    /// Tween duration for animated moves (ms).
    tween_ms: u64,
    /// Explicit target rect to apply, if present.
    apply_target: Option<(f64, f64, f64, f64)>,
    /// Grid parameters to compute target rect, if present.
    apply_grid: Option<(u32, u32, u32, u32)>,
    // Async-frame state
    /// Last observed window position.
    last_pos: Option<(f64, f64)>,
    /// Last observed window size.
    last_size: Option<(f64, f64)>,
    /// Desired position requested by the test.
    desired_pos: Option<(f64, f64)>,
    /// Desired size requested by the test.
    desired_size: Option<(f64, f64)>,
    /// Time at which to apply pending placement.
    apply_after: Option<Instant>,
    // Tween state
    /// Whether a tween animation is currently active.
    tween_active: bool,
    /// Tween start time.
    tween_start: Option<Instant>,
    /// Tween end time.
    tween_end: Option<Instant>,
    /// Starting position for tween.
    tween_from_pos: Option<(f64, f64)>,
    /// Starting size for tween.
    tween_from_size: Option<(f64, f64)>,
    /// Target position for tween.
    tween_to_pos: Option<(f64, f64)>,
    /// Target size for tween.
    tween_to_size: Option<(f64, f64)>,
    /// Suppress processing of window events while applying changes.
    suppress_events: bool,
    /// Number of upcoming window events triggered by helper writes to ignore.
    pending_suppressed_events: u8,
    /// Optional 2x2 slot for placement.
    slot: Option<u8>,
    /// Optional grid spec for placement.
    grid: Option<(u32, u32, u32, u32)>,
    /// Optional explicit initial size.
    size: Option<(f64, f64)>,
    /// Optional explicit initial position.
    pos: Option<(f64, f64)>,
    /// Optional label text to display.
    label_text: Option<String>,
    /// Optional minimum content size.
    min_size: Option<(f64, f64)>,
    /// Fatal error encountered during setup.
    error: Option<String>,
    /// Start minimized if requested.
    start_minimized: bool,
    /// Start zoomed (macOS “zoomed”) if requested.
    start_zoomed: bool,
    /// Make the panel non-movable if requested.
    panel_nonmovable: bool,
    /// Make the panel non-resizable if requested.
    panel_nonresizable: bool,
    /// Attach a modal sheet to the helper window if requested.
    attach_sheet: bool,
    // Optional: round requested sizes to nearest multiples
    /// Width rounding step for requested sizes.
    step_w: f64,
    /// Height rounding step for requested sizes.
    step_h: f64,
    /// Shutdown flag toggled by the harness to request exit.
    shutdown: Arc<AtomicBool>,
    /// Whether the helper requested termination.
    should_exit: bool,
    /// Active quirk list applied to this helper window.
    quirks: Vec<Quirk>,
    /// Placement options for raise/minimize behaviour.
    place: PlaceOptions,
    /// Pending smart raise plan, if any.
    raise_plan: Option<RaisePlan>,
    /// Pending placement retry state, if any.
    place_plan: Option<PlacePlan>,
}

struct RaisePlan {
    start: Instant,
    deadline: Instant,
    next_attempt: Instant,
    last_raise: Option<Instant>,
    click_attempted: bool,
}

struct PlacePlan {
    cols: u32,
    rows: u32,
    col: u32,
    row: u32,
    options: Option<PlaceAttemptOptions>,
    attempts: u32,
    next_attempt: Instant,
}

impl HelperApp {
    fn has_quirk(&self, quirk: Quirk) -> bool {
        self.quirks.contains(&quirk)
    }

    /// Ensure the next `count` window events triggered by helper writes are ignored.
    fn queue_suppressed_events(&mut self, count: u8) {
        self.pending_suppressed_events = self.pending_suppressed_events.saturating_add(count);
    }

    /// Suppress downstream events after issuing geometry writes.
    fn queue_apply_events(&mut self, writes: u8) {
        if writes == 0 {
            return;
        }
        const SUPPRESSED_EVENT_FANOUT: u8 = 2;
        let scaled = writes.saturating_mul(SUPPRESSED_EVENT_FANOUT);
        self.queue_suppressed_events(scaled);
    }

    fn diag_tag(&self) -> String {
        format!(
            "{}/{} quirks=[{}]",
            self.scenario_slug.as_ref(),
            self.window_label.as_ref(),
            format_quirks(&self.quirks)
        )
    }

    /// Build a helper app with the provided configuration snapshot.
    pub(super) fn new(params: HelperParams) -> Self {
        let HelperParams {
            title,
            scenario_slug,
            window_label,
            time_ms,
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
            panel_nonresizable,
            attach_sheet,
            quirks,
            place,
            shutdown,
        } = params;
        Self {
            window: None,
            title,
            scenario_slug,
            window_label,
            deadline: Instant::now() + config::ms(time_ms.max(1000)),
            post_create_ready_at: None,
            delay_setframe_ms,
            delay_apply_ms,
            tween_ms,
            apply_target,
            apply_grid,
            last_pos: None,
            last_size: None,
            desired_pos: None,
            desired_size: None,
            apply_after: None,
            tween_active: false,
            tween_start: None,
            tween_end: None,
            tween_from_pos: None,
            tween_from_size: None,
            tween_to_pos: None,
            tween_to_size: None,
            suppress_events: false,
            pending_suppressed_events: 0,
            slot,
            grid,
            size,
            pos,
            label_text,
            min_size,
            error: None,
            start_minimized,
            start_zoomed,
            panel_nonmovable,
            panel_nonresizable,
            attach_sheet,
            step_w: step_size.map(|s| s.0).unwrap_or(0.0),
            step_h: step_size.map(|s| s.1).unwrap_or(0.0),
            shutdown,
            should_exit: false,
            quirks,
            place,
            raise_plan: None,
            place_plan: None,
        }
    }

    /// Return any captured fatal error and clear it from state.
    pub(super) fn take_error(&mut self) -> Option<String> {
        self.error.take()
    }

    /// Whether the helper has requested termination.
    pub(super) fn should_finish(&self) -> bool {
        self.should_exit
    }

    /// Request termination without tearing down the shared event loop.
    fn request_exit(&mut self, elwt: &ActiveEventLoop) {
        if self.should_exit {
            return;
        }
        if let Some(window) = self.window.take() {
            window.set_visible(false);
        }
        self.close_helper_nswindow();
        self.should_exit = true;
        elwt.set_control_flow(ControlFlow::WaitUntil(Instant::now()));
    }

    /// Request AppKit to close the helper NSWindow and wait for teardown.
    fn close_helper_nswindow(&self) {
        if let Some(mtm) = objc2_foundation::MainThreadMarker::new() {
            let app = objc2_app_kit::NSApplication::sharedApplication(mtm);
            let mut close_requested = false;
            let windows = app.windows();
            for window in windows.iter() {
                if self.matches_helper_title(&window) {
                    window.close();
                    close_requested = true;
                }
            }
            drop(windows);
            if close_requested {
                self.wait_for_appkit_teardown(&app);
            }
        }
    }

    /// Return true when the candidate window title matches the helper title.
    fn matches_helper_title(&self, window: &NSWindow) -> bool {
        let title = window.title();
        autoreleasepool(|pool| unsafe { title.to_str(pool) == self.title })
    }

    /// Poll AppKit until the helper window is confirmed closed or the timeout expires.
    fn wait_for_appkit_teardown(&self, app: &objc2_app_kit::NSApplication) {
        let timeout = Duration::from_millis(200);
        let start = Instant::now();
        loop {
            let windows = app.windows();
            let still_open = windows
                .iter()
                .any(|window| self.matches_helper_title(&window));
            drop(windows);
            if !still_open {
                break;
            }
            if start.elapsed() >= timeout {
                debug!(
                    "winhelper: timed out waiting for helper window '{}' to close",
                    self.title
                );
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
    }

    /// Lazily create the helper window when the event loop is ready.
    fn ensure_window(&mut self, elwt: &ActiveEventLoop) -> Result<(), ()> {
        if self.window.is_some() {
            return Ok(());
        }
        let win = match self.try_create_window(elwt) {
            Ok(w) => w,
            Err(e) => {
                self.error = Some(format!("winhelper: failed to create window: {}", e));
                self.request_exit(elwt);
                return Err(());
            }
        };
        self.activate_app();
        self.apply_min_size_if_requested();
        self.apply_nonmovable_if_requested();
        self.initial_placement(&win);
        self.post_create_ready_at =
            Some(Instant::now() + config::ms(config::INPUT_DELAYS.window_registration_delay_ms));
        self.window = Some(win);
        self.ensure_auto_unminimize();
        if let Some(active) = self.window.as_ref() {
            active.set_visible(true);
            active.focus_window();
        }
        Ok(())
    }

    fn complete_post_create_if_ready(&mut self, now: Instant) {
        let Some(ready_at) = self.post_create_ready_at else {
            return;
        };
        if now < ready_at {
            return;
        }
        self.capture_initial_geometry();
        self.arm_delayed_apply_if_configured();
        let _ = self.attach_sheet;
        self.apply_initial_state_options();
        self.add_centered_label();
        self.post_create_ready_at = None;
    }

    /// Duration until the next scheduled helper wake-up.
    pub(super) fn next_wakeup_timeout(&self) -> Duration {
        let mut next = self.deadline;
        if let Some(apply_after) = self.apply_after {
            next = next.min(apply_after);
        }
        if let Some(ready) = self.post_create_ready_at {
            next = next.min(ready);
        }
        if let Some(plan) = &self.raise_plan {
            next = next.min(plan.next_attempt.min(plan.deadline));
        }
        if let Some(plan) = &self.place_plan {
            next = next.min(plan.next_attempt);
        }
        let now = Instant::now();
        let max_slice = Duration::from_millis(16);
        match next.checked_duration_since(now) {
            Some(duration) if !duration.is_zero() => duration.min(max_slice),
            _ => Duration::ZERO,
        }
    }

    /// Create the helper window with initial attributes.
    fn try_create_window(&self, elwt: &ActiveEventLoop) -> Result<Window, String> {
        use winit::dpi::LogicalSize;
        let attrs = Window::default_attributes()
            .with_title(self.title.clone())
            .with_visible(true)
            .with_decorations(false)
            .with_inner_size(LogicalSize::new(
                self.size
                    .map(|s| s.0)
                    .unwrap_or(config::HELPER_WINDOW.width_px),
                self.size
                    .map(|s| s.1)
                    .unwrap_or(config::HELPER_WINDOW.height_px),
            ));
        elwt.create_window(attrs).map_err(|e| e.to_string())
    }

    /// Bring the application to the foreground on resume.
    fn activate_app(&self) {
        if let Some(mtm) = objc2_foundation::MainThreadMarker::new() {
            let app = objc2_app_kit::NSApplication::sharedApplication(mtm);
            #[allow(deprecated)]
            app.activateIgnoringOtherApps(true);
            unsafe {
                app.unhide(None);
            }
        }
    }

    /// Enforce the configured minimum content size, if any.
    fn apply_min_size_if_requested(&self) {
        if let Some((min_w, min_h)) = self.min_size
            && let Some(mtm) = objc2_foundation::MainThreadMarker::new()
        {
            use objc2_foundation::NSSize;
            let app = objc2_app_kit::NSApplication::sharedApplication(mtm);
            for w in app.windows().iter() {
                let t = w.title();
                let is_match = autoreleasepool(|pool| unsafe { t.to_str(pool) == self.title });
                if is_match {
                    unsafe {
                        w.setMinSize(NSSize::new(min_w, min_h));
                        w.setContentMinSize(NSSize::new(min_w, min_h));
                    }
                    break;
                }
            }
        }
    }

    /// Make the panel non-movable if configured.
    fn apply_nonmovable_if_requested(&self) {
        if self.panel_nonmovable
            && let Some(mtm) = objc2_foundation::MainThreadMarker::new()
        {
            let app = objc2_app_kit::NSApplication::sharedApplication(mtm);
            let windows = app.windows();
            for w in windows.iter() {
                let t = w.title();
                let is_match = autoreleasepool(|pool| unsafe { t.to_str(pool) == self.title });
                if is_match {
                    w.setMovable(false);
                    break;
                }
            }
        }
    }

    /// Make the panel non-resizable if configured.
    fn apply_nonresizable_if_requested(&self) {
        if self.panel_nonresizable
            && let Some(mtm) = objc2_foundation::MainThreadMarker::new()
        {
            use objc2_app_kit::NSWindowStyleMask;
            let app = objc2_app_kit::NSApplication::sharedApplication(mtm);
            let windows = app.windows();
            for w in windows.iter() {
                let t = w.title();
                let is_match = autoreleasepool(|pool| unsafe { t.to_str(pool) == self.title });
                if is_match {
                    let mut mask = w.styleMask();
                    mask.remove(NSWindowStyleMask::Resizable);
                    mask.insert(NSWindowStyleMask::Titled);
                    mask.insert(NSWindowStyleMask::Closable);
                    mask.insert(NSWindowStyleMask::Miniaturizable);
                    w.setStyleMask(mask);
                    unsafe {
                        w.setHidesOnDeactivate(false);
                    }
                    w.makeKeyAndOrderFront(None);
                    break;
                }
            }
        }
    }

    fn smart_raise_window(&mut self, deadline: Duration) {
        let now = Instant::now();
        self.raise_plan = Some(RaisePlan {
            start: now,
            deadline: now + deadline,
            next_attempt: now,
            last_raise: None,
            click_attempted: false,
        });
    }

    fn drive_raise_plan(&mut self, now: Instant) {
        let Some(mut plan) = self.raise_plan.take() else {
            return;
        };
        if now >= plan.deadline {
            debug!(tag = %self.diag_tag(), "smart_raise_deadline_elapsed");
            return;
        }
        if now < plan.next_attempt {
            self.raise_plan = Some(plan);
            return;
        }
        let pid = id() as i32;
        if let Some(target) = self.resolve_world_window() {
            let wid = target.window_id();
            let should_raise = plan
                .last_raise
                .map(|ts| now.duration_since(ts) >= Duration::from_millis(160))
                .unwrap_or(true);
            if should_raise {
                match mac_winops::raise_window(pid, wid) {
                    Ok(()) => {}
                    Err(mac_winops::Error::MainThread) => {
                        let _ = mac_winops::request_raise_window(pid, wid);
                    }
                    Err(err) => debug!(
                        tag = %self.diag_tag(),
                        pid,
                        id = wid,
                        error = %err,
                        "smart_raise_raise_failed"
                    ),
                }
                plan.last_raise = Some(now);
            }

            if self.window_visible_in_world(pid, wid) {
                debug!(tag = %self.diag_tag(), "smart_raise_visible_confirmed");
                return;
            }

            if !plan.click_attempted && now.duration_since(plan.start) >= Duration::from_millis(200)
            {
                plan.click_attempted = mac_winops::click_window_center(pid, &self.title);
                if plan.click_attempted {
                    debug!(tag = %self.diag_tag(), "smart_raise_click_issued");
                }
            }
        }

        plan.next_attempt = now + Duration::from_millis(40);
        self.raise_plan = Some(plan);
    }

    fn window_visible_in_world(&self, pid: i32, wid: u32) -> bool {
        match world::list_windows() {
            Ok(windows) => windows
                .iter()
                .any(|w| w.pid == pid && w.id == wid && w.is_on_screen && w.on_active_space),
            Err(err) => {
                debug!(tag = %self.diag_tag(), error = %err, "smart_raise_visibility_check_failed");
                false
            }
        }
    }

    fn drive_place_plan(&mut self, now: Instant) {
        let Some(mut plan) = self.place_plan.take() else {
            return;
        };
        if plan.attempts >= 120 {
            debug!("winhelper: world placement giving up after retries");
            return;
        }
        if now < plan.next_attempt {
            self.place_plan = Some(plan);
            return;
        }
        if let Some(target) = self.resolve_world_window() {
            let pid = target.pid();
            match world::place_window(
                target,
                plan.cols,
                plan.rows,
                plan.col,
                plan.row,
                plan.options.clone(),
            ) {
                Ok(_) => {
                    if self.verify_grid_cell(pid, plan.cols, plan.rows, plan.col, plan.row) {
                        return;
                    }
                }
                Err(err) => {
                    debug!(
                        "winhelper: world placement attempt {} failed: {}",
                        plan.attempts, err
                    );
                }
            }
        }

        plan.attempts += 1;
        plan.next_attempt = now + Duration::from_millis(20);
        self.place_plan = Some(plan);
    }

    /// Poll the world snapshot to resolve the helper window's identifier.
    fn resolve_world_window(&self) -> Option<WorldWindowId> {
        let pid = id() as i32;
        match world::list_windows() {
            Ok(windows) => windows
                .into_iter()
                .find(|w| w.pid == pid && w.title == self.title)
                .map(|w| WorldWindowId::new(w.pid, w.id)),
            Err(err) => {
                tracing::debug!("winhelper: world snapshot failed: {}", err);
                None
            }
        }
    }

    /// Perform the initial placement of the window.
    fn initial_placement(&mut self, win: &Window) {
        use winit::dpi::LogicalPosition;
        let pid = id() as i32;
        match self.place.raise {
            RaiseStrategy::None | RaiseStrategy::KeepFrontWindow => {
                debug!(
                    "winhelper: skip ensure_frontmost (raise={:?})",
                    self.place.raise
                );
            }
            RaiseStrategy::AppActivate => {
                if let Err(err) = world::ensure_frontmost(
                    pid,
                    &self.title,
                    3,
                    config::INPUT_DELAYS.retry_delay_ms,
                ) {
                    debug!(
                        "winhelper: ensure_frontmost failed pid={} title='{}': {}",
                        pid, self.title, err
                    );
                }
            }
            RaiseStrategy::SmartRaise { deadline } => {
                self.smart_raise_window(deadline);
            }
        }
        if let Some((cols, rows, col, row)) = self.grid {
            self.try_world_place(cols, rows, col, row, None);
        } else if let Some(slot) = self.slot {
            let (col, row) = match slot {
                1 => (0, 0),
                2 => (1, 0),
                3 => (0, 1),
                _ => (1, 1),
            };
            self.try_world_place(2, 2, col, row, None);
        } else if let Some((x, y)) = self.pos {
            win.set_outer_position(LogicalPosition::new(x, y));
        } else if let Some(mtm) = objc2_foundation::MainThreadMarker::new() {
            // Fallback: bottom-right corner at a fixed small size on main screen.
            use objc2_app_kit::NSScreen;
            let margin: f64 = config::HELPER_WINDOW.margin_px;
            if let Some(scr) = NSScreen::mainScreen(mtm) {
                let vf = scr.visibleFrame();
                let w = config::HELPER_WINDOW.width_px;
                let x = (vf.origin.x + vf.size.width - w - margin).max(0.0);
                let y = (vf.origin.y + margin).max(0.0);
                win.set_outer_position(LogicalPosition::new(x, y));
            }
        }
        // Apply style tweaks after initial placement to avoid interfering with it.
        self.apply_nonresizable_if_requested();
        self.apply_nonmovable_if_requested();
    }

    /// Retry world placement until the helper window is visible to the world snapshot.
    fn try_world_place(
        &mut self,
        cols: u32,
        rows: u32,
        col: u32,
        row: u32,
        options: Option<&PlaceAttemptOptions>,
    ) {
        self.place_plan = Some(PlacePlan {
            cols,
            rows,
            col,
            row,
            options: options.cloned(),
            attempts: 0,
            next_attempt: Instant::now(),
        });
    }

    /// Confirm the helper window occupies the requested grid cell using
    /// anchored semantics (position matches exactly; size may exceed the
    /// cell because of minimums or non-resizable windows).
    fn verify_grid_cell(&self, pid: i32, cols: u32, rows: u32, col: u32, row: u32) -> bool {
        if let Some(((x, y), (w, h))) = mac_winops::ax_window_frame(pid, &self.title)
            && let Some(vf) = screen::visible_frame_containing_point(x, y)
        {
            let expected = mac_winops::cell_rect(vf, cols, rows, col, row);
            let eps = config::PLACE.eps;
            let pos_ok = (x - expected.x).abs() <= eps && (y - expected.y).abs() <= eps;
            let size_ok = (w + eps) >= expected.w && (h + eps) >= expected.h;
            return pos_ok && size_ok;
        }
        false
    }

    /// Capture the starting geometry used by delayed/tweened placement logic.
    fn capture_initial_geometry(&mut self) {
        let Some(win) = self.window.as_ref() else {
            return;
        };
        let scale = win.scale_factor();
        if let Ok(p) = win.outer_position() {
            let lp = p.to_logical::<f64>(scale);
            self.last_pos = Some((lp.x, lp.y));
        }
        let isz = win.inner_size();
        let lsz = isz.to_logical::<f64>(scale);
        self.last_size = Some((lsz.width, lsz.height));
    }

    /// Arm delayed application if explicitly configured.
    fn arm_delayed_apply_if_configured(&mut self) {
        if self.delay_apply_ms > 0 {
            self.apply_after = Some(Instant::now() + config::ms(self.delay_apply_ms));
            debug!(
                "winhelper: armed delayed-apply +{}ms target={:?} grid={:?}",
                self.delay_apply_ms, self.apply_target, self.apply_grid
            );
        }
    }

    /// Apply initial zoom/minimize state if requested.
    fn apply_initial_state_options(&self) {
        if let Some(mtm) = objc2_foundation::MainThreadMarker::new() {
            let app = objc2_app_kit::NSApplication::sharedApplication(mtm);
            let windows = app.windows();
            for w in windows.iter() {
                let t = w.title();
                let is_match = autoreleasepool(|pool| unsafe { t.to_str(pool) == self.title });
                if is_match {
                    unsafe {
                        if self.start_zoomed && !w.isZoomed() {
                            w.performZoom(None);
                        }
                        if self.start_minimized && !w.isMiniaturized() {
                            w.miniaturize(None);
                        }
                    }
                    break;
                }
            }
        }
    }

    fn window_is_minimized(&self) -> bool {
        if let Some(mtm) = objc2_foundation::MainThreadMarker::new() {
            let app = objc2_app_kit::NSApplication::sharedApplication(mtm);
            let windows = app.windows();
            for w in windows.iter() {
                let title = w.title();
                let is_match = autoreleasepool(|pool| unsafe { title.to_str(pool) == self.title });
                if is_match {
                    return w.isMiniaturized();
                }
            }
        }
        false
    }

    fn ensure_auto_unminimize(&mut self) {
        if self.place.minimized != MinimizedPolicy::AutoUnminimize {
            return;
        }
        if !self.window_is_minimized() {
            return;
        }
        if let Some(win) = self.window.as_ref() {
            win.set_minimized(false);
            win.focus_window();
        }
    }

    fn apply_ax_rounding_override(&self) {
        if !self.has_quirk(Quirk::AxRounding) {
            return;
        }
        if let Some(target) = self.resolve_world_window()
            && let Some(win) = self.window.as_ref()
        {
            let scale = win.scale_factor();
            if let Ok(pos) = win.outer_position() {
                let lp = pos.to_logical::<f64>(scale);
                let size = win.inner_size().to_logical::<f64>(scale);
                let rect = Rect {
                    x: lp.x.floor(),
                    y: lp.y.floor(),
                    w: size.width.floor(),
                    h: size.height.floor(),
                };
                let props = AxProps {
                    role: None,
                    subrole: None,
                    can_set_pos: Some(true),
                    can_set_size: Some(true),
                    frame: Some(rect),
                    minimized: Some(false),
                    fullscreen: Some(false),
                    visible: Some(true),
                    zoomed: Some(false),
                };
                crate::test_api::set_ax_props(target.pid(), target.window_id(), props);
            }
        }
    }

    /// Add a large centered label to the content view.
    fn add_centered_label(&self) {
        if let Some(mtm) = objc2_foundation::MainThreadMarker::new() {
            let app = objc2_app_kit::NSApplication::sharedApplication(mtm);
            let windows = app.windows();
            for w in windows.iter() {
                let title = w.title();
                let is_match = autoreleasepool(|pool| unsafe { title.to_str(pool) == self.title });
                if is_match {
                    if let Some(view) = w.contentView() {
                        use objc2::rc::Retained;
                        use objc2_app_kit::{NSColor, NSFont, NSTextAlignment, NSTextField};
                        use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};
                        let label_str = self.compute_label_text();
                        let ns = NSString::from_str(&label_str);
                        let label: Retained<NSTextField> =
                            unsafe { NSTextField::labelWithString(&ns, mtm) };
                        let vframe = view.frame();
                        let vw = vframe.size.width;
                        let vh = vframe.size.height;
                        let base = vw.min(vh) * 0.35;
                        let font = unsafe { NSFont::boldSystemFontOfSize(base) };
                        unsafe { label.setFont(Some(&font)) };
                        unsafe { label.setAlignment(NSTextAlignment::Center) };
                        let color = unsafe { NSColor::whiteColor() };
                        unsafe { label.setTextColor(Some(&color)) };
                        let margin_x = 8.0;
                        let margin_y = 8.0;
                        let lw = (vw - 2.0 * margin_x).max(10.0);
                        let lh = (vh - 2.0 * margin_y).max(20.0);
                        let lx = vframe.origin.x + margin_x;
                        let ly = vframe.origin.y + margin_y;
                        let frame = NSRect::new(NSPoint::new(lx, ly), NSSize::new(lw, lh));
                        unsafe { label.setFrame(frame) };
                        unsafe { view.addSubview(&label) };
                    }
                    break;
                }
            }
        }
    }

    /// Handle a `WindowEvent::Moved`.
    fn on_moved(&mut self, new_pos: PhysicalPosition<i32>) {
        use winit::dpi::LogicalPosition;
        debug!("winhelper: moved event: x={} y={}", new_pos.x, new_pos.y);
        if self.pending_suppressed_events > 0 {
            self.pending_suppressed_events -= 1;
            if let Some(win) = self.window.as_ref() {
                let scale = win.scale_factor();
                let lp = new_pos.to_logical::<f64>(scale);
                self.last_pos = Some((lp.x, lp.y));
            }
            return;
        }
        let intercept = (self.delay_setframe_ms > 0
            || self.delay_apply_ms > 0
            || (self.tween_ms > 0 && !self.tween_active))
            && !self.suppress_events;
        if intercept {
            if let Some(win) = self.window.as_ref() {
                let scale = win.scale_factor();
                let lp = new_pos.to_logical::<f64>(scale);
                if self.last_pos.is_none()
                    && let Ok(p0) = win.outer_position()
                {
                    let p0l = p0.to_logical::<f64>(scale);
                    self.last_pos = Some((p0l.x, p0l.y));
                }
                self.desired_pos = Some((lp.x, lp.y));
                debug!(
                    "winhelper: intercept move -> desired=({:.1},{:.1}) last={:?}",
                    lp.x, lp.y, self.last_pos
                );
                if let Some((x, y)) = self.last_pos {
                    self.suppress_events = true;
                    let mut reverted = false;
                    if let Some(win_ref) = self.window.as_ref() {
                        win_ref.set_outer_position(LogicalPosition::new(x, y));
                        reverted = true;
                    }
                    self.suppress_events = false;
                    if reverted {
                        self.queue_suppressed_events(1);
                    }
                }
                if self.tween_ms > 0 {
                    if self.delay_apply_ms > 0
                        && (self.apply_target.is_some() || self.apply_grid.is_some())
                    {
                        self.apply_after = Some(Instant::now() + config::ms(self.delay_apply_ms));
                    } else {
                        let now = Instant::now();
                        self.ensure_tween_started_pos(now);
                        self.tween_to_pos = self.desired_pos;
                        self.apply_after = Some(now);
                    }
                } else if self.delay_apply_ms > 0 {
                    self.apply_after = Some(Instant::now() + config::ms(self.delay_apply_ms));
                    debug!(
                        "winhelper: scheduled apply_after at +{}ms (delay_apply)",
                        self.delay_apply_ms
                    );
                } else {
                    self.apply_after = Some(Instant::now() + config::ms(self.delay_setframe_ms));
                    debug!(
                        "winhelper: scheduled apply_after at +{}ms (delay_setframe)",
                        self.delay_setframe_ms
                    );
                }
            }
        } else if !self.suppress_events
            && let Some(win) = self.window.as_ref()
        {
            let scale = win.scale_factor();
            let lp = new_pos.to_logical::<f64>(scale);
            self.last_pos = Some((lp.x, lp.y));
            debug!("winhelper: track move -> last=({:.1},{:.1})", lp.x, lp.y);
        }
    }

    fn cycle_focus_to_sibling_if_needed(&self) {
        if self.place.raise != RaiseStrategy::KeepFrontWindow {
            return;
        }
        if !self.has_quirk(Quirk::RaiseCyclesToSibling) {
            return;
        }
        let pid = id() as i32;
        match world::list_windows() {
            Ok(windows) => {
                let slug_fragment = format!("[{}::", self.scenario_slug.as_ref());
                if let Some(sibling) =
                    select_sibling_for_cycle(&windows, pid, &self.title, &slug_fragment)
                {
                    let sibling_label = parse_decorated_label(&sibling.title)
                        .unwrap_or("?")
                        .to_string();
                    self.raise_sibling(sibling.pid, sibling.id, sibling_label);
                }
            }
            Err(err) => debug!(
                tag = %self.diag_tag(),
                error = %err,
                "failed to list windows during focus cycle"
            ),
        }
    }

    fn raise_sibling(&self, pid: i32, id: u32, sibling_label: String) {
        let sibling_tag = format!("{}/{}", self.scenario_slug.as_ref(), sibling_label);
        match mac_winops::raise_window(pid, id) {
            Ok(()) => {
                debug!(
                    tag = %self.diag_tag(),
                    sibling = %sibling_tag,
                    "cycled focus to sibling"
                );
            }
            Err(err) => debug!(
                tag = %self.diag_tag(),
                sibling = %sibling_tag,
                error = %err,
                "failed to raise sibling during focus cycle"
            ),
        }
    }

    /// Handle a `WindowEvent::Resized`.
    fn on_resized(&mut self, new_size: PhysicalSize<u32>) {
        use winit::dpi::LogicalSize;
        debug!(
            "winhelper: resized event: w={} h={}",
            new_size.width, new_size.height
        );
        if self.pending_suppressed_events > 0 {
            self.pending_suppressed_events -= 1;
            if let Some(win) = self.window.as_ref() {
                let scale = win.scale_factor();
                let lsz = new_size.to_logical::<f64>(scale);
                self.last_size = Some((lsz.width, lsz.height));
            }
            return;
        }
        let intercept = (self.delay_setframe_ms > 0
            || self.delay_apply_ms > 0
            || (self.tween_ms > 0 && !self.tween_active))
            && !self.suppress_events;
        if intercept {
            if let Some(win) = self.window.as_ref() {
                let scale = win.scale_factor();
                let lsz = new_size.to_logical::<f64>(scale);
                if self.last_size.is_none() {
                    let s0 = win.inner_size().to_logical::<f64>(scale);
                    self.last_size = Some((s0.width, s0.height));
                }
                self.desired_size = Some((lsz.width, lsz.height));
                debug!(
                    "winhelper: intercept resize -> desired=({:.1},{:.1}) last={:?}",
                    lsz.width, lsz.height, self.last_size
                );
                if let Some((w, h)) = self.last_size {
                    self.suppress_events = true;
                    let mut reverted = false;
                    if let Some(win_ref) = self.window.as_ref() {
                        let _ = win_ref.request_inner_size(LogicalSize::new(w, h));
                        reverted = true;
                    }
                    self.suppress_events = false;
                    if reverted {
                        self.queue_suppressed_events(1);
                    }
                }
                if self.tween_ms > 0 {
                    if self.delay_apply_ms > 0
                        && (self.apply_target.is_some() || self.apply_grid.is_some())
                    {
                        self.apply_after = Some(Instant::now() + config::ms(self.delay_apply_ms));
                    } else {
                        let now = Instant::now();
                        self.ensure_tween_started_size(now);
                        self.tween_to_size = self.desired_size;
                        self.apply_after = Some(now);
                    }
                } else if self.delay_apply_ms > 0 {
                    self.apply_after = Some(Instant::now() + config::ms(self.delay_apply_ms));
                    debug!(
                        "winhelper: scheduled apply_after at +{}ms (delay_apply)",
                        self.delay_apply_ms
                    );
                } else {
                    self.apply_after = Some(Instant::now() + config::ms(self.delay_setframe_ms));
                    debug!(
                        "winhelper: scheduled apply_after at +{}ms (delay_setframe)",
                        self.delay_setframe_ms
                    );
                }
            }
        } else if !self.suppress_events
            && let Some(win) = self.window.as_ref()
        {
            let scale = win.scale_factor();
            let lsz = new_size.to_logical::<f64>(scale);
            self.last_size = Some((lsz.width, lsz.height));
            debug!(
                "winhelper: track resize -> last=({:.1},{:.1})",
                lsz.width, lsz.height
            );
        }
    }

    /// Handle a `WindowEvent::Focused`.
    fn on_focused(&self, focused: bool) {
        debug!(title = %self.title, focused, "winhelper: focus event");
        if let Some(mtm) = objc2_foundation::MainThreadMarker::new() {
            let app = objc2_app_kit::NSApplication::sharedApplication(mtm);
            let windows = app.windows();
            for w in windows.iter() {
                let title = w.title();
                let is_match = autoreleasepool(|pool| unsafe { title.to_str(pool) == self.title });
                if is_match {
                    let color = unsafe {
                        if focused {
                            objc2_app_kit::NSColor::systemBlueColor()
                        } else {
                            objc2_app_kit::NSColor::controlBackgroundColor()
                        }
                    };
                    w.setBackgroundColor(Some(&color));
                    break;
                }
            }
        }
        if focused {
            self.cycle_focus_to_sibling_if_needed();
        }
    }
    /// Return the visible frame of the screen containing the given window.
    fn active_visible_frame_for_window(&self, win: &Window) -> (f64, f64, f64, f64) {
        if let Some(mtm) = objc2_foundation::MainThreadMarker::new() {
            use objc2_app_kit::NSScreen;
            let scale = win.scale_factor();
            let p = win
                .outer_position()
                .ok()
                .map(|p| p.to_logical::<f64>(scale))
                .unwrap_or(LogicalPosition::new(0.0, 0.0));
            let mut chosen: Option<(f64, f64, f64, f64)> = None;
            for s in NSScreen::screens(mtm).iter() {
                let fr = s.visibleFrame();
                let sx = fr.origin.x;
                let sy = fr.origin.y;
                let sw = fr.size.width;
                let sh = fr.size.height;
                if p.x >= sx && p.x <= sx + sw && p.y >= sy && p.y <= sy + sh {
                    chosen = Some((sx, sy, sw, sh));
                    break;
                }
            }
            if let Some(v) = chosen {
                return v;
            }
            if let Some(scr) = NSScreen::mainScreen(mtm) {
                let r = scr.visibleFrame();
                return (r.origin.x, r.origin.y, r.size.width, r.size.height);
            }
        }
        (0.0, 0.0, 1440.0, 900.0)
    }

    /// Compute the rectangle for a tile within a grid on the active screen.
    fn grid_rect(
        &self,
        win: &Window,
        cols: u32,
        rows: u32,
        col: u32,
        row: u32,
    ) -> (f64, f64, f64, f64) {
        let (vf_x, vf_y, vf_w, vf_h) = self.active_visible_frame_for_window(win);
        let c = cols.max(1) as f64;
        let r = rows.max(1) as f64;
        let tile_w = (vf_w / c).floor().max(1.0);
        let tile_h = (vf_h / r).floor().max(1.0);
        let rem_w = vf_w - tile_w * (cols as f64);
        let rem_h = vf_h - tile_h * (rows as f64);
        let tx = vf_x + tile_w * (col as f64);
        let tw = if col == cols.saturating_sub(1) {
            tile_w + rem_w
        } else {
            tile_w
        };
        let ty = vf_y + tile_h * (row as f64);
        let th = if row == rows.saturating_sub(1) {
            tile_h + rem_h
        } else {
            tile_h
        };
        (tx, ty, tw, th)
    }

    /// Compute the label text to display in the helper window.
    /// Helper selectors are inlined at call sites to avoid borrow conflicts.
    fn compute_label_text(&self) -> String {
        if let Some(ref t) = self.label_text {
            return t.clone();
        }
        if let Some((cols, rows, col, row)) = self.grid {
            if cols == 2 && rows == 2 {
                return match (col, row) {
                    (0, 0) => "TL".into(),
                    (1, 0) => "TR".into(),
                    (0, 1) => "BL".into(),
                    _ => "BR".into(),
                };
            }
            return self
                .clean_title_label()
                .unwrap_or_else(|| self.fallback_label_letter());
        }
        if let Some(slot) = self.slot {
            return match slot {
                1 => "TL".into(),
                2 => "TR".into(),
                3 => "BL".into(),
                _ => "BR".into(),
            };
        }
        self.clean_title_label()
            .unwrap_or_else(|| self.fallback_label_letter())
    }

    fn clean_title_label(&self) -> Option<String> {
        let raw = self.title.trim();
        if raw.is_empty() {
            return None;
        }
        let base = raw
            .split_once('[')
            .map(|(head, _)| head.trim())
            .unwrap_or(raw);
        let normalized = base
            .trim()
            .trim_end_matches([':', '['])
            .replace(['_', '-'], " ");
        if normalized.trim().is_empty() {
            None
        } else {
            Some(normalized.trim().to_string())
        }
    }

    fn fallback_label_letter(&self) -> String {
        let label = self.window_label.as_ref();
        let fallback_char = label
            .chars()
            .find(|c| c.is_ascii_alphabetic())
            .map(|c| c.to_ascii_uppercase())
            .unwrap_or('A');
        fallback_char.to_string()
    }

    /// Ensure tween state is initialized for position changes.
    fn ensure_tween_started_pos(&mut self, now: Instant) {
        if !self.tween_active {
            self.tween_active = true;
            self.tween_start = Some(now);
            self.tween_end = Some(now + config::ms(self.tween_ms));
            self.tween_from_pos = self.last_pos;
        }
    }

    /// Ensure tween state is initialized for size changes.
    fn ensure_tween_started_size(&mut self, now: Instant) {
        if !self.tween_active {
            self.tween_active = true;
            self.tween_start = Some(now);
            self.tween_end = Some(now + config::ms(self.tween_ms));
            self.tween_from_size = self.last_size;
        }
    }

    /// Optionally round a size to the configured step.
    fn rounded_size(&self, w: f64, h: f64) -> (f64, f64) {
        if self.step_w > 0.0 && self.step_h > 0.0 {
            (
                (w / self.step_w).round() * self.step_w,
                (h / self.step_h).round() * self.step_h,
            )
        } else {
            (w, h)
        }
    }

    /// Select the target rectangle to apply (explicit target, grid, or desired geometry).
    fn select_apply_target(&self, win: &Window) -> Option<TargetRect> {
        if let Some((x, y, w, h)) = self.apply_target {
            return Some(((x, y), (w, h), "target"));
        }
        if let Some((c, r, ic, ir)) = self.apply_grid {
            let (tx, ty, tw, th) = self.grid_rect(win, c, r, ic, ir);
            return Some(((tx, ty), (tw, th), "grid"));
        }
        let pos = self.desired_pos.or(self.last_pos);
        let size = self.desired_size.or(self.last_size);
        match (pos, size) {
            (Some(p), Some(s)) => Some((p, s, "desired")),
            _ => None,
        }
    }

    /// Initialize tween destination from a target rectangle.
    fn set_tween_target_from(&mut self, target: TargetRect) {
        let ((x, y), (w, h), kind) = target;
        self.tween_to_pos = Some((x, y));
        self.tween_to_size = Some((w, h));
        debug!(
            "winhelper: tween-start ({}) -> ({:.1},{:.1},{:.1},{:.1})",
            kind, x, y, w, h
        );
    }

    /// Reset tween bookkeeping when abandoning an animation.
    fn clear_tween_state(&mut self) {
        self.tween_active = false;
        self.tween_start = None;
        self.tween_end = None;
        self.tween_from_pos = None;
        self.tween_from_size = None;
        self.tween_to_pos = None;
        self.tween_to_size = None;
    }

    /// Compute tween progress in the range [0.0, 1.0].
    fn tween_progress(&self, now: Instant) -> f64 {
        let start = match self.tween_start {
            Some(s) => s,
            None => return 1.0,
        };
        let end = match self.tween_end {
            Some(e) => e,
            None => return 1.0,
        };
        let total = end.saturating_duration_since(start);
        if total.as_millis() == 0 {
            1.0
        } else {
            let elapsed = now.saturating_duration_since(start).as_secs_f64();
            (elapsed / total.as_secs_f64()).clamp(0.0, 1.0)
        }
    }

    /// Interpolate position and size based on tween progress `t`.
    fn tween_interpolate(&self, t: f64) -> (f64, f64, f64, f64) {
        let (mut nx, mut ny) = self.last_pos.unwrap_or((0.0, 0.0));
        let (mut nw, mut nh) = self.last_size.unwrap_or((
            config::HELPER_WINDOW.width_px,
            config::HELPER_WINDOW.height_px,
        ));
        if let (Some((fx, fy)), Some((tx, ty))) = (self.tween_from_pos, self.tween_to_pos) {
            nx = fx + (tx - fx) * t;
            ny = fy + (ty - fy) * t;
        }
        if let (Some((fw, fh)), Some((tw, th))) = (self.tween_from_size, self.tween_to_size) {
            nw = fw + (tw - fw) * t;
            nh = fh + (th - fh) * t;
        }
        (nx, ny, nw, nh)
    }

    /// Revert minor drift that may occur while testing async placement.
    fn revert_drift_if_needed(&mut self) {
        let mut suppressed = 0u8;
        if let (Some((lx, ly)), Some((lw, lh))) = (self.last_pos, self.last_size)
            && let Some(win) = self.window.as_ref()
        {
            let scale = win.scale_factor();
            let p = win
                .outer_position()
                .ok()
                .map(|pos| pos.to_logical::<f64>(scale));
            let s = win.inner_size().to_logical::<f64>(scale);
            if let Some(p) = p {
                let dx = (p.x - lx).abs();
                let dy = (p.y - ly).abs();
                let dw = (s.width - lw).abs();
                let dh = (s.height - lh).abs();
                let width_span = lw.abs().max(1.0);
                let height_span = lh.abs().max(1.0);
                let has_explicit_target = self.apply_target.is_some() || self.apply_grid.is_some();
                let big_pos_shift = dx >= width_span / 2.0 || dy >= height_span / 2.0;
                let big_size_shift = dw >= width_span / 2.0 || dh >= height_span / 2.0;
                let adopt_external = (big_pos_shift || big_size_shift) && !has_explicit_target;
                if adopt_external {
                    debug!(
                        "winhelper: adopt external geometry dx={:.1} dy={:.1} dw={:.1} dh={:.1}",
                        dx, dy, dw, dh
                    );
                    self.last_pos = Some((p.x, p.y));
                    self.last_size = Some((s.width, s.height));
                    self.desired_pos = None;
                    self.desired_size = None;
                    self.apply_target = None;
                    self.apply_grid = None;
                    self.apply_after = None;
                    self.clear_tween_state();
                    return;
                }
                if dx > 0.5 || dy > 0.5 || dw > 0.5 || dh > 0.5 {
                    debug!(
                        "winhelper: revert drift dx={:.1} dy={:.1} dw={:.1} dh={:.1}",
                        dx, dy, dw, dh
                    );
                    self.suppress_events = true;
                    let _ = win.request_inner_size(LogicalSize::new(lw, lh));
                    win.set_outer_position(LogicalPosition::new(lx, ly));
                    self.suppress_events = false;
                    suppressed = suppressed.saturating_add(2);
                }
            }
        }
        self.queue_apply_events(suppressed);
    }

    /// Apply a single tween step, updating window position/size.
    fn apply_tween_step(&mut self) {
        self.ensure_auto_unminimize();
        if should_skip_apply_for_minimized(&self.quirks, self.window_is_minimized()) {
            debug!("winhelper: skip tween apply while minimized");
            self.apply_after = None;
            return;
        }
        let now = Instant::now();
        if let Some(win) = self.window.as_ref() {
            debug!("winhelper: apply_tween_step start");
            let target = self.select_apply_target(win);
            if !self.tween_active {
                self.tween_active = true;
                self.tween_start = Some(now);
                self.tween_end = Some(now + config::ms(self.tween_ms));
                self.tween_from_pos = self.last_pos;
                self.tween_from_size = self.last_size;
                if let Some(target) = target {
                    self.set_tween_target_from(target);
                }
            }
        }
        let mut size_applied = false;
        let mut pos_applied = false;
        if let Some(win2) = self.window.as_ref() {
            let t = self.tween_progress(now);
            let (nx, ny, nw, nh) = self.tween_interpolate(t);
            let (rw, rh) = self.rounded_size(nw, nh);
            let _ = win2.request_inner_size(LogicalSize::new(rw, rh));
            win2.set_outer_position(LogicalPosition::new(nx, ny));
            self.last_size = Some((rw, rh));
            self.last_pos = Some((nx, ny));
            size_applied = true;
            pos_applied = true;
        }
        if size_applied {
            self.queue_apply_events(1);
        }
        if pos_applied {
            self.queue_apply_events(1);
        }
        if self.tween_start.is_some() && self.tween_end.is_some() {
            let t_done = self.tween_progress(Instant::now());
            if (t_done - 1.0).abs() < f64::EPSILON {
                if let Some((w, h)) = self.tween_to_size {
                    let (_rw, _rh) = self.rounded_size(w, h);
                    self.last_size = Some((w, h));
                }
                self.last_pos = self.tween_to_pos.or(self.last_pos);
                self.tween_active = false;
                self.tween_start = None;
                self.tween_end = None;
                self.tween_from_pos = None;
                self.tween_from_size = None;
                self.tween_to_pos = None;
                self.tween_to_size = None;
                self.apply_after = None;
            } else {
                self.apply_after = Some(now + config::ms(16));
            }
        }
        self.apply_ax_rounding_override();
    }

    /// Apply target geometry immediately without tweening.
    fn apply_immediate(&mut self) {
        self.ensure_auto_unminimize();
        if should_skip_apply_for_minimized(&self.quirks, self.window_is_minimized()) {
            debug!("winhelper: skip immediate apply while minimized");
            self.apply_after = None;
            return;
        }
        let mut suppressed = 0u8;
        if let Some(win) = self.window.as_ref() {
            use winit::dpi::{LogicalPosition, LogicalSize};
            if let Some((x, y, w, h)) = self.apply_target {
                let (rw, rh) = self.rounded_size(w, h);
                if !self.panel_nonresizable {
                    let _ = win.request_inner_size(LogicalSize::new(rw, rh));
                    suppressed = suppressed.saturating_add(1);
                }
                win.set_outer_position(LogicalPosition::new(x, y));
                self.last_pos = Some((x, y));
                self.last_size = Some((rw, rh));
                suppressed = suppressed.saturating_add(1);
                debug!(
                    "winhelper: explicit apply (explicit) -> ({:.1},{:.1},{:.1},{:.1})",
                    x, y, rw, rh
                );
                // Clear explicit targets after applying so we don't re-issue
                // redundant AX operations every event loop tick.
                self.apply_target = None;
                self.apply_grid = None;
            } else if let Some((cols, rows, col, row)) = self.apply_grid {
                let (tx, ty, tw, th) = self.grid_rect(win, cols, rows, col, row);
                let (rw, rh) = self.rounded_size(tw, th);
                if !self.panel_nonresizable {
                    let _ = win.request_inner_size(LogicalSize::new(rw, rh));
                    suppressed = suppressed.saturating_add(1);
                }
                win.set_outer_position(LogicalPosition::new(tx, ty));
                self.last_pos = Some((tx, ty));
                self.last_size = Some((rw, rh));
                suppressed = suppressed.saturating_add(1);
                debug!(
                    "winhelper: explicit apply (grid) -> ({:.1},{:.1},{:.1},{:.1})",
                    tx, ty, rw, rh
                );
                // Grid-driven targets only need a single apply; subsequent
                // drift corrections rely on `last_pos`/`last_size`.
                self.apply_grid = None;
                self.apply_target = None;
            } else {
                let desired_size = self.desired_size.take();
                let desired_pos = self.desired_pos.take();
                if let Some((w, h)) = desired_size {
                    let (rw, rh) = self.rounded_size(w, h);
                    if !self.panel_nonresizable {
                        let _ = win.request_inner_size(LogicalSize::new(rw, rh));
                        suppressed = suppressed.saturating_add(1);
                    }
                    self.last_size = Some((rw, rh));
                }
                if let Some((x, y)) = desired_pos {
                    win.set_outer_position(LogicalPosition::new(x, y));
                    self.last_pos = Some((x, y));
                    suppressed = suppressed.saturating_add(1);
                }
                if desired_size.is_some() || desired_pos.is_some() {
                    debug!("winhelper: applied desired pos/size");
                }
            }
        }
        self.queue_apply_events(suppressed);
        // Ensure we clear any pending apply-after state after applying.
        self.apply_after = None;
        self.apply_ax_rounding_override();
    }

    /// Apply placement when the apply deadline has been reached.
    fn process_apply_ready(&mut self) {
        if self.window.is_none() {
            return;
        }
        debug!(
            "winhelper: process_apply_ready apply_after={:?}",
            self.apply_after
        );
        self.suppress_events = true;
        if self.tween_ms > 0 {
            self.apply_tween_step();
        } else {
            self.apply_immediate();
        }
        self.suppress_events = false;
    }
}

impl ApplicationHandler for HelperApp {
    fn resumed(&mut self, elwt: &ActiveEventLoop) {
        let now = Instant::now();
        if self.ensure_window(elwt).is_ok() {
            self.complete_post_create_if_ready(now);
        }
    }
    fn window_event(&mut self, elwt: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => self.request_exit(elwt),
            WindowEvent::Moved(pos) => self.on_moved(pos),
            WindowEvent::Resized(sz) => self.on_resized(sz),
            WindowEvent::Focused(f) => self.on_focused(f),
            _ => {}
        }
    }
    fn about_to_wait(&mut self, elwt: &ActiveEventLoop) {
        if self.shutdown.load(Ordering::SeqCst) {
            self.request_exit(elwt);
            return;
        }
        let now = Instant::now();
        if now >= self.deadline {
            self.request_exit(elwt);
            return;
        }
        if self.window.is_none() && self.ensure_window(elwt).is_err() {
            return;
        }
        if self.window.is_some() {
            self.complete_post_create_if_ready(now);
        }
        self.drive_raise_plan(now);
        self.drive_place_plan(now);

        if self.post_create_ready_at.is_some() {
            self.revert_drift_if_needed();
        } else {
            match self.apply_after {
                Some(when) if now < when => self.revert_drift_if_needed(),
                _ => self.process_apply_ready(),
            }
        }

        let wake_after = self.next_wakeup_timeout();
        elwt.set_control_flow(ControlFlow::WaitUntil(now + wake_after));
    }
}

impl HelperParams {
    pub(super) fn from_config(title: String, config: HelperConfig) -> Self {
        let HelperConfig {
            time_ms,
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
            scenario_slug,
            window_label,
            start_minimized,
            start_zoomed,
            panel_nonmovable,
            panel_nonresizable,
            attach_sheet,
            quirks,
            place,
            shutdown,
        } = config;

        Self {
            title,
            scenario_slug,
            window_label,
            time_ms,
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
            panel_nonresizable,
            attach_sheet,
            quirks,
            place,
            shutdown,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn select_sibling_finds_matching_window() {
        let slug_fragment = "[demo::";
        let windows = vec![
            test_window(10, "hotki helper [demo::primary]"),
            test_window(10, "hotki helper [demo::sibling]"),
        ];
        let sibling =
            select_sibling_for_cycle(&windows, 10, "hotki helper [demo::primary]", slug_fragment)
                .expect("sibling window");
        assert_eq!(sibling.title, "hotki helper [demo::sibling]");
    }

    #[test]
    fn select_sibling_skips_non_matching_pid() {
        let slug_fragment = "[demo::";
        let windows = vec![
            test_window(11, "hotki helper [demo::primary]"),
            test_window(10, "hotki helper [other::sibling]"),
        ];
        assert!(
            select_sibling_for_cycle(&windows, 10, "hotki helper [demo::primary]", slug_fragment)
                .is_none()
        );
    }

    #[test]
    fn skip_apply_helper_respects_quirk_and_minimize_state() {
        assert!(should_skip_apply_for_minimized(
            &[Quirk::IgnoreMoveIfMinimized],
            true
        ));
        assert!(!should_skip_apply_for_minimized(
            &[Quirk::IgnoreMoveIfMinimized],
            false
        ));
        assert!(!should_skip_apply_for_minimized(&[Quirk::AxRounding], true));
    }

    fn test_window(pid: i32, title: &str) -> WindowInfo {
        WindowInfo {
            app: "TestApp".into(),
            title: title.into(),
            pid,
            id: 42,
            pos: None,
            space: None,
            layer: 0,
            focused: false,
            is_on_screen: true,
            on_active_space: true,
        }
    }
}
