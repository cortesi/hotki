//! Async placement smoketest: helper delays applying window frame changes by ~50ms.
//! Verifies that engine polling/settle logic converges within the default budget.

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use objc2_app_kit::NSScreen;
use objc2_foundation::MainThreadMarker;

use crate::{
    config,
    error::{Error, Result},
    process::{HelperWindowBuilder, ManagedChild},
};

fn find_window_id(
    pid: i32,
    title: &str,
    timeout_ms: u64,
    poll_ms: u64,
) -> Option<mac_winops::WindowId> {
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    while Instant::now() < deadline {
        if let Some(w) = mac_winops::list_windows()
            .into_iter()
            .find(|w| w.pid == pid && w.title == title)
        {
            return Some(w.id);
        }
        std::thread::sleep(Duration::from_millis(poll_ms));
    }
    None
}

fn visible_frame_containing_point(x: f64, y: f64) -> Option<(f64, f64, f64, f64)> {
    let mtm = MainThreadMarker::new()?;
    for s in NSScreen::screens(mtm).iter() {
        let fr = s.visibleFrame();
        let sx = fr.origin.x;
        let sy = fr.origin.y;
        let sw = fr.size.width;
        let sh = fr.size.height;
        if x >= sx && x <= sx + sw && y >= sy && y <= sy + sh {
            return Some((sx, sy, sw, sh));
        }
    }
    if let Some(scr) = NSScreen::mainScreen(mtm) {
        let r = scr.visibleFrame();
        return Some((r.origin.x, r.origin.y, r.size.width, r.size.height));
    }
    if let Some(s) = NSScreen::screens(mtm).iter().next() {
        let r = s.visibleFrame();
        return Some((r.origin.x, r.origin.y, r.size.width, r.size.height));
    }
    None
}

fn cell_rect(
    vf_x: f64,
    vf_y: f64,
    vf_w: f64,
    vf_h: f64,
    cols: u32,
    rows: u32,
    col: u32,
    row: u32,
) -> (f64, f64, f64, f64) {
    let c = cols.max(1) as f64;
    let r = rows.max(1) as f64;
    let tile_w = (vf_w / c).floor().max(1.0);
    let tile_h = (vf_h / r).floor().max(1.0);
    let rem_w = vf_w - tile_w * (cols as f64);
    let rem_h = vf_h - tile_h * (rows as f64);

    let x = vf_x + tile_w * (col as f64);
    let w = if col == cols.saturating_sub(1) {
        tile_w + rem_w
    } else {
        tile_w
    };
    let y = vf_y + tile_h * (row as f64);
    let h = if row == rows.saturating_sub(1) {
        tile_h + rem_h
    } else {
        tile_h
    };
    (x, y, w, h)
}

fn approx(a: f64, b: f64, eps: f64) -> bool {
    (a - b).abs() <= eps
}

fn wait_for_expected_frame(
    pid: i32,
    title: &str,
    expected: (f64, f64, f64, f64),
    eps: f64,
    timeout_ms: u64,
    poll_ms: u64,
) -> bool {
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    while Instant::now() < deadline {
        if let Some(((px, py), (w, h))) = mac_winops::ax_window_frame(pid, title)
            && approx(px, expected.0, eps)
            && approx(py, expected.1, eps)
            && approx(w, expected.2, eps)
            && approx(h, expected.3, eps)
        {
            return true;
        }
        std::thread::sleep(Duration::from_millis(poll_ms));
    }
    false
}

pub fn run_place_async_test(timeout_ms: u64, with_logs: bool) -> Result<()> {
    let cols = 2u32;
    let rows = 2u32;
    let col = 1u32; // BR
    let row = 1u32;
    let now_pre = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let helper_title = format!(
        "hotki smoketest: place-async {}-{}",
        std::process::id(),
        now_pre
    );

    // Build a minimal hotki config so the backend is up (but we will call
    // mac-winops directly for placement to reduce orchestration flakiness).
    let ron_config: String =
        "(keys: [], style: (hud: (mode: hide)), server: (exit_if_no_clients: true))\n".into();

    let cfg = crate::test_runner::TestConfig::new(timeout_ms)
        .with_logs(with_logs)
        .with_temp_config(ron_config);

    crate::test_runner::TestRunner::new("place_async", cfg)
        .with_setup(|ctx| {
            ctx.launch_hotki()?;
            if let Some(sess) = ctx.session.as_ref() {
                let sock = sess.socket_path().to_string();
                let start = std::time::Instant::now();
                let mut inited = crate::server_drive::init(&sock);
                while !inited && start.elapsed() < std::time::Duration::from_millis(3000) {
                    std::thread::sleep(std::time::Duration::from_millis(50));
                    inited = crate::server_drive::init(&sock);
                }
            }
            let _ = crate::server_drive::wait_for_ident("g", crate::config::BINDING_GATE_DEFAULT_MS);
            let _ = crate::server_drive::wait_for_ident("b", crate::config::BINDING_GATE_DEFAULT_MS);
            Ok(())
        })
        .with_execute(move |ctx| {
            // Spawn helper with delayed setFrame behavior
            let title = helper_title.clone();
            let helper_time = ctx
                .config
                .timeout_ms
                .saturating_add(config::HELPER_WINDOW_EXTRA_TIME_MS);
            // Compute expected grid rect now based on the main or first screen;
            // the helper spawns on the main screen by default.
            let expected = {
                let mtm = MainThreadMarker::new()
                    .ok_or_else(|| Error::InvalidState("no main thread marker".into()))?;
                let (vf_x, vf_y, vf_w, vf_h) = if let Some(scr) = objc2_app_kit::NSScreen::mainScreen(mtm) {
                    let r = scr.visibleFrame();
                    (r.origin.x, r.origin.y, r.size.width, r.size.height)
                } else if let Some(s) = objc2_app_kit::NSScreen::screens(mtm).iter().next() {
                    let r = s.visibleFrame();
                    (r.origin.x, r.origin.y, r.size.width, r.size.height)
                } else {
                    (0.0, 0.0, 1440.0, 900.0)
                };
                cell_rect(vf_x, vf_y, vf_w, vf_h, cols, rows, col, row)
            };
            let mut helper: ManagedChild = HelperWindowBuilder::new(title.clone())
                .with_time_ms(helper_time)
                .with_label_text("A")
                .with_delay_apply_grid(50, cols, rows, col, row)
                .spawn()?;

            // Wait for visibility
            if !crate::tests::helpers::wait_for_window_visible(
                helper.pid,
                &title,
                std::cmp::min(ctx.config.timeout_ms, config::HIDE_FIRST_WINDOW_MAX_MS),
                config::PLACE_POLL_MS,
            ) {
                return Err(Error::InvalidState("helper window not visible".into()));
            }

            // Resolve window id and ensure frontmost by best-effort activation
            let _ = find_window_id(helper.pid, &title, 2000, config::PLACE_POLL_MS)
                .ok_or_else(|| Error::InvalidState("Failed to resolve helper CGWindowId".into()))?;
            crate::tests::helpers::ensure_frontmost(
                helper.pid,
                &title,
                5,
                config::RETRY_DELAY_MS,
            );

            // Compute expected rect for (1,1) at current screen
            let (vf_x, vf_y, vf_w, vf_h) = if let Some((px, py)) =
                mac_winops::ax_window_position(helper.pid, &title)
                && let Some(vf) = visible_frame_containing_point(px, py)
            {
                vf
            } else {
                return Err(Error::InvalidState("Failed to resolve screen visibleFrame".into()));
            };
            let expected = cell_rect(vf_x, vf_y, vf_w, vf_h, cols, rows, col, row);

            // Trigger placement directly via mac-winops (focused-for-pid)
            // This exercises the exact placement code-path while avoiding
            // orchestrator races.
            let _ = mac_winops::place_grid_focused(helper.pid, cols, rows, col, row)
                .map_err(|e| Error::SpawnFailed(format!("place_grid_focused failed: {}", e)))?;
            let ok = wait_for_expected_frame(
                helper.pid,
                &title,
                expected,
                config::PLACE_EPS,
                config::PLACE_STEP_TIMEOUT_MS,
                config::PLACE_POLL_MS,
            );
            if !ok {
                let actual = mac_winops::ax_window_frame(helper.pid, &title)
                    .map(|((ax, ay), (aw, ah))| (ax, ay, aw, ah));
                return Err(Error::SpawnFailed(match actual {
                    Some((ax, ay, aw, ah)) => format!(
                        "placement mismatch (async) (expected x={:.1} y={:.1} w={:.1} h={:.1}; actual x={:.1} y={:.1} w={:.1} h={:.1})",
                        expected.0, expected.1, expected.2, expected.3, ax, ay, aw, ah
                    ),
                    None => format!(
                        "placement mismatch (async) (expected x={:.1} y={:.1} w={:.1} h={:.1}; actual frame unavailable)",
                        expected.0, expected.1, expected.2, expected.3
                    ),
                }));
            }

            let _ = helper.kill_and_wait();
            Ok(())
        })
        .with_teardown(|ctx, _| {
            ctx.shutdown();
            Ok(())
        })
        .run()
}
