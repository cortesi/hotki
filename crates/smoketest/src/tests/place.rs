//! Grid placement smoketest.
//!
//! What this verifies
//! - Placing a window into every cell of an `cols × rows` grid positions it to
//!   the exact cell rectangle derived from the screen’s AppKit visible frame.
//! - The test computes the expected rectangle for each cell and compares the
//!   observed AX frame against it.
//!
//! Acceptance criteria
//! - For every `(col,row)` cell, after sending the bound key, the helper
//!   window’s `(x, y, w, h)` matches the expected cell rectangle within
//!   `PLACE_EPS` tolerance and before `PLACE_STEP_TIMEOUT_MS` expires.
//! - If the helper CGWindowId cannot be found, the screen’s visible frame
//!   cannot be resolved, or any cell fails to match within tolerance/time, the
//!   test fails with a detailed mismatch error (expected vs. actual).
//!
//! Notes
//! - The HUD is hidden; a `g` binding raises the helper before each placement
//!   to ensure the correct target pid.
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use objc2_app_kit::NSScreen;
use objc2_foundation::MainThreadMarker;

use super::helpers::wait_for_frontmost_title;
use crate::{
    config,
    error::{Error, Result},
    server_drive,
    test_runner::{TestConfig, TestRunner},
    ui_interaction::send_key,
};

/// Find the CG `WindowId` for a window identified by `(pid, title)`.
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

/// Compute AppKit visible frame for the screen containing point `(x,y)`.
/// Mirrors mac-winops internal logic to pick the screen and return `visibleFrame`.
fn visible_frame_containing_point(x: f64, y: f64) -> Option<(f64, f64, f64, f64)> {
    let mtm = MainThreadMarker::new()?;
    // Prefer screen whose visibleFrame contains the point
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
    // Fallbacks to main screen, then first screen
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

#[allow(clippy::too_many_arguments)]
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
    // Match mac-winops::cell_rect (top-left origin; row 0 is top)
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

pub fn run_place_test(timeout_ms: u64, with_logs: bool) -> Result<()> {
    // Bind all actions directly at the top level (no nested modes, HUD hidden).
    // Keys:
    //   g → raise helper by title (noexit)
    //   1..N (and letters thereafter) → place into each grid cell (row-major)
    let cols = config::PLACE_COLS;
    let rows = config::PLACE_ROWS;
    let mut entries = String::new();
    let mut key_for_cell: Vec<(u32, u32, char)> = Vec::new();
    let mut keycode = 1usize;
    for row in 0..rows {
        for col in 0..cols {
            let ch = if keycode <= 9 {
                char::from_digit(keycode as u32, 10).unwrap()
            } else {
                // After '9', continue with letters a, b, c ...
                (b'a' + (keycode as u8 - 10)) as char
            };
            entries.push_str(&format!(
                "            (\"{}\", \"({}, {})\", place(grid({}, {}), at({}, {}))),\n",
                ch, col, row, cols, rows, col, row
            ));
            key_for_cell.push((col, row, ch));
            keycode += 1;
        }
    }
    // Precompute helper title and embed a raise binding that targets it.
    let now_pre = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let helper_title = format!("hotki smoketest: place {}-{}", std::process::id(), now_pre);
    let ron_config: String = format!(
        "(\n    keys: [\n        (\"g\", \"raise\", raise(title: \"{}\"), (noexit: true)),\n{}    ],\n    style: (hud: (mode: hide)),\n    server: (exit_if_no_clients: true),\n)\n",
        helper_title, entries
    );
    let config = TestConfig::new(timeout_ms)
        .with_logs(with_logs)
        // HUD remains hidden; bind top-level keys directly
        .with_temp_config(ron_config);

    TestRunner::new("place_test", config)
        .with_setup(|ctx| {
            ctx.launch_hotki()?;
            // Connect to backend and gate on bindings being present
            if let Some(sess) = ctx.session.as_ref() {
                let sock = sess.socket_path().to_string();
                let start = std::time::Instant::now();
                let mut inited = server_drive::init(&sock);
                while !inited && start.elapsed() < std::time::Duration::from_millis(3000) {
                    std::thread::sleep(std::time::Duration::from_millis(50));
                    inited = server_drive::init(&sock);
                }
            }
            // Wait for a couple of identifiers we expect at top-level
            let _ = server_drive::wait_for_ident("g", crate::config::BINDING_GATE_DEFAULT_MS);
            let _ = server_drive::wait_for_ident("1", crate::config::BINDING_GATE_DEFAULT_MS);
            Ok(())
        })
        .with_execute(move |ctx| {
            // Spawn helper window using the title embedded in the config
            let title = helper_title.clone();
            let helper_time = ctx
                .config
                .timeout_ms
                .saturating_add(config::HELPER_WINDOW_EXTRA_TIME_MS);
            let mut helper = crate::tests::helpers::spawn_helper_visible(
                title.clone(),
                helper_time,
                std::cmp::min(ctx.config.timeout_ms, config::HIDE_FIRST_WINDOW_MAX_MS),
                config::PLACE_POLL_MS,
                "P",
            )?;

            // Resolve CG window id
            let _wid = find_window_id(helper.pid, &title, 2000, config::PLACE_POLL_MS)
                .ok_or_else(|| Error::InvalidState("Failed to resolve helper CGWindowId".into()))?;

            // Ensure helper is frontmost via backend raise binding; actively wait for it
            send_key("g");
            let _ = wait_for_frontmost_title(&title, config::WAIT_FIRST_WINDOW_MS);

            // Iterate all grid cells in row-major order (top-left is (0,0))
            for (col, row, key) in key_for_cell {
                    // Re-raise helper to ensure engine targets the right pid and wait until frontmost
                    send_key("g");
                    let _ = wait_for_frontmost_title(&title, config::WAIT_FIRST_WINDOW_MS);
                    // Recompute visible frame based on current AX position (matches backend logic)
                    let (vf_x, vf_y, vf_w, vf_h) = if let Some((px, py)) =
                        mac_winops::ax_window_position(helper.pid, &title)
                        && let Some(vf) = visible_frame_containing_point(px, py)
                    {
                        vf
                    } else {
                        return Err(Error::InvalidState("Failed to resolve screen visibleFrame".into()));
                    };

                    // Compute expected cell rect
                    let (ex, ey, ew, eh) = cell_rect(vf_x, vf_y, vf_w, vf_h, cols, rows, col, row);

                    // Drive backend: send the key for this cell directly (no nested modes)
                    let key_str = key.to_string();
                    send_key(&key_str);

                    // Wait for expected frame within tolerance
                    let ok = wait_for_expected_frame(
                        helper.pid,
                        &title,
                        (ex, ey, ew, eh),
                        config::PLACE_EPS,
                        config::PLACE_STEP_TIMEOUT_MS,
                        config::PLACE_POLL_MS,
                    );
                    if !ok {
                        let actual = mac_winops::ax_window_frame(helper.pid, &title)
                            .map(|((ax, ay), (aw, ah))| (ax, ay, aw, ah));
                        return Err(Error::SpawnFailed(match actual {
                            Some((ax, ay, aw, ah)) => format!(
                                "placement mismatch at col={}, row={} (expected x={:.1} y={:.1} w={:.1} h={:.1}; actual x={:.1} y={:.1} w={:.1} h={:.1})",
                                col, row, ex, ey, ew, eh, ax, ay, aw, ah
                            ),
                            None => format!(
                                "placement mismatch at col={}, row={} (expected x={:.1} y={:.1} w={:.1} h={:.1}; actual frame unavailable)",
                                col, row, ex, ey, ew, eh
                            ),
                        }));
                    }
                }

            // Kill helper explicitly to exercise teardown
            let _ = helper.kill_and_wait();
            Ok(())
        })
        .with_teardown(|ctx, _| {
            ctx.shutdown();
            Ok(())
        })
        .run()
}
