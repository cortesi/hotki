//! Terminal-style placement guard: simulate resize increments and ensure that once
//! the origin reaches the correct cell corner, subsequent frames never move it.

use std::{
    cmp,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use crate::{
    config,
    error::{Error, Result},
    helper_window::{HelperWindowBuilder, ManagedChild, ensure_frontmost, wait_for_window_visible},
    test_runner::{TestConfig, TestRunner},
    tests::fixtures,
};

/// Lightweight sample of a window frame at a point in time.
#[derive(Clone, Copy)]
struct Sample {
    /// X origin in screen coordinates.
    x: f64,
    /// Y origin in screen coordinates.
    y: f64,
    /// Width in points.
    w: f64,
    /// Height in points.
    h: f64,
}

/// Approximate float equality within `eps` absolute tolerance.
fn approx(a: f64, b: f64, eps: f64) -> bool {
    (a - b).abs() <= eps
}

/// Run terminal-style placement guard smoketest with step size rounding.
pub fn run_place_term_test(timeout_ms: u64, _with_logs: bool) -> Result<()> {
    // Case: 3x1, left cell (0,0) — representative of terminal placement.
    let cols = 3u32;
    let rows = 1u32;
    let col = 0u32;
    let row = 0u32;
    let helper_title = config::test_title("place-term");

    // Minimal hotki config (server up; we drive mac-winops directly here).
    let ron_config: String =
        "(keys: [], style: (hud: (mode: hide)), server: (exit_if_no_clients: true))\n".into();

    let cfg = TestConfig::new(timeout_ms)
        .with_logs(true)
        .with_temp_config(ron_config);

    TestRunner::new("place_term", cfg)
        .with_setup(|ctx| {
            ctx.launch_hotki()?;
            ctx.ensure_rpc_ready(&[])?;
            Ok(())
        })
        .with_execute(move |ctx| {
            // Spawn helper with step-size rounding to simulate terminal increments
            let title = helper_title.clone();
            let helper_time = ctx
                .config
                .timeout_ms
                .saturating_add(config::HELPER_WINDOW.extra_time_ms);
            let mut helper: ManagedChild = HelperWindowBuilder::new(title.clone())
                .with_time_ms(helper_time)
                .with_label_text("TM")
                .with_step_size(9.0, 18.0)
                .spawn_inherit_io()?;

            // Wait until visible and make sure it's frontmost
            if !wait_for_window_visible(
                helper.pid,
                &title,
                cmp::min(ctx.config.timeout_ms, config::HIDE.first_window_max_ms),
                config::PLACE.poll_ms,
            ) {
                return Err(Error::InvalidState("helper window not visible".into()));
            }
            ensure_frontmost(helper.pid, &title, 5, config::INPUT_DELAYS.retry_delay_ms);

            // Compute expected visibleFrame and cell target
            let ((ax, ay), _) = mac_winops::ax_window_frame(helper.pid, &title)
                .ok_or_else(|| Error::InvalidState("No AX frame for helper".into()))?;
            let vf = fixtures::visible_frame_containing_point(ax, ay)
                .ok_or_else(|| Error::InvalidState("Failed to resolve visibleFrame".into()))?;
            let target = fixtures::cell_rect(vf, cols, rows, col, row);
            let right = target.x + target.w;
            let top = target.y + target.h;

            // Sampler: collect AX frame timeline in the background
            let samples: Arc<Mutex<Vec<Sample>>> = Arc::new(Mutex::new(Vec::new()));
            let done = Arc::new(AtomicBool::new(false));
            let s_clone = samples.clone();
            let d_clone = done.clone();
            let title_clone = title;
            let pid_clone = helper.pid;
            thread::spawn(move || {
                let deadline = Instant::now()
                    + Duration::from_millis(config::PLACE.step_timeout_ms.saturating_add(1500));
                while !d_clone.load(Ordering::SeqCst) && Instant::now() < deadline {
                    if let Some(((x, y), (w, h))) =
                        mac_winops::ax_window_frame(pid_clone, &title_clone)
                        && let Ok(mut guard) = s_clone.lock()
                    {
                        guard.push(Sample { x, y, w, h });
                    }
                    thread::sleep(Duration::from_millis(25));
                }
            });

            // Execute placement
            mac_winops::place_grid_focused(helper.pid, cols, rows, col, row)
                .map_err(|e| Error::SpawnFailed(format!("place_grid_focused failed: {}", e)))?;
            // Allow tail to capture final state, then stop sampler
            thread::sleep(Duration::from_millis(300));
            done.store(true, Ordering::SeqCst);

            // Analyze timeline
            let samples = samples
                .lock()
                .map_err(|_| Error::InvalidState("sampler poisoned".into()))?;
            if samples.is_empty() {
                return Err(Error::InvalidState("no samples collected".into()));
            }
            let eps = 4.0_f64; // allow small rounding while sampling
            // Find last sample anchored to cell edges
            let mut last_idx: Option<usize> = None;
            for (i, s) in samples.iter().enumerate().rev() {
                let left_ok = approx(s.x, target.x, eps);
                let right_ok = approx(s.x + s.w, right, eps);
                let bottom_ok = approx(s.y, target.y, eps);
                let top_ok = approx(s.y + s.h, top, eps);
                if (left_ok || right_ok) && (bottom_ok || top_ok) {
                    last_idx = Some(i);
                    break;
                }
            }
            let last_idx = last_idx.ok_or_else(|| {
                Error::InvalidState("no anchored sample found in timeline".into())
            })?;
            let last = &samples[last_idx];
            let left_ok = approx(last.x, target.x, eps);
            let bottom_ok = approx(last.y, target.y, eps);

            // Find earliest sample that matches final anchoring and assert no drift after
            let mut latch_idx: Option<usize> = None;
            for (i, s) in samples.iter().enumerate() {
                let htop = s.y + s.h;
                let hright = s.x + s.w;
                let horiz_ok = if left_ok {
                    approx(s.x, target.x, eps)
                } else {
                    approx(hright, right, eps)
                };
                let vert_ok = if bottom_ok {
                    approx(s.y, target.y, eps)
                } else {
                    approx(htop, top, eps)
                };
                if horiz_ok && vert_ok {
                    latch_idx = Some(i);
                    break;
                }
            }
            let li = latch_idx.ok_or_else(|| {
                Error::InvalidState("never observed final anchoring during placement".into())
            })?;
            for s in &samples[li..] {
                let htop = s.y + s.h;
                let hright = s.x + s.w;
                let horiz_ok = if left_ok {
                    approx(s.x, target.x, eps)
                } else {
                    approx(hright, right, eps)
                };
                let vert_ok = if bottom_ok {
                    approx(s.y, target.y, eps)
                } else {
                    approx(htop, top, eps)
                };
                if !(horiz_ok && vert_ok) {
                    return Err(Error::InvalidState(format!(
                        "anchoring drifted after latch: saw=({:.1},{:.1},{:.1},{:.1})",
                        s.x, s.y, s.w, s.h
                    )));
                }
            }

            if let Err(_e) = helper.kill_and_wait() {}
            Ok(())
        })
        .with_teardown(|ctx, _| {
            ctx.shutdown();
            Ok(())
        })
        .run()
}
