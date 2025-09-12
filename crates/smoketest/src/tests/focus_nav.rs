//! Focus navigation smoketest using the new `focus(dir)` action.

use std::time::{SystemTime, UNIX_EPOCH};

use objc2_app_kit::NSScreen;
use objc2_foundation::MainThreadMarker;
use tracing::info;

use super::helpers::{wait_for_frontmost_title, wait_for_windows_visible};
use crate::{
    config,
    error::{Error, Result},
    process::HelperWindowBuilder,
    test_runner::{TestConfig, TestRunner},
    ui_interaction::send_key,
};

// Placement handled via server by driving config bindings (no direct WinOps here).

const EPS: f64 = 2.0;

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
    if let Some(s) = NSScreen::screens(mtm).iter().next() {
        let fr = s.visibleFrame();
        return Some((fr.origin.x, fr.origin.y, fr.size.width, fr.size.height));
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
    // Top-left origin; row 0 is top
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

fn approx(a: f64, b: f64) -> bool {
    (a - b).abs() <= EPS
}

fn assert_frontmost_cell(expected_title: &str, col: u32, row: u32) -> Result<()> {
    let front = mac_winops::frontmost_window()
        .ok_or_else(|| Error::InvalidState("No frontmost CG window".into()))?;
    if front.title != expected_title {
        return Err(Error::FocusNotObserved {
            timeout_ms: 1000,
            expected: format!("{} (frontmost: {})", expected_title, front.title),
        });
    }
    // Verify AX frame roughly matches the expected grid cell
    let ((x, y), (w, h)) = mac_winops::ax_window_frame(front.pid, &front.title)
        .ok_or_else(|| Error::InvalidState("AX frame for frontmost not available".into()))?;
    let (vf_x, vf_y, vf_w, vf_h) = visible_frame_containing_point(x, y)
        .ok_or_else(|| Error::InvalidState("No visibleFrame for frontmost".into()))?;
    let (ex, ey, ew, eh) = cell_rect(vf_x, vf_y, vf_w, vf_h, 2, 2, col, row);
    if !(approx(x, ex) && approx(y, ey) && approx(w, ew) && approx(h, eh)) {
        return Err(Error::InvalidState(format!(
            "frontmost not in expected cell ({},{}): got x={:.1} y={:.1} w={:.1} h={:.1} | expected x={:.1} y={:.1} w={:.1} h={:.1}",
            col, row, x, y, w, h, ex, ey, ew, eh
        )));
    }
    info!(
        "focus-nav: frontmost='{}' at cell({}, {}) ok; pos=({:.1},{:.1}) size=({:.1},{:.1})",
        expected_title, col, row, x, y, w, h
    );
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn find_cell_for_frame(
    vf_x: f64,
    vf_y: f64,
    vf_w: f64,
    vf_h: f64,
    cols: u32,
    rows: u32,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    eps: f64,
) -> Option<(u32, u32)> {
    for row in 0..rows {
        for col in 0..cols {
            let (ex, ey, ew, eh) = cell_rect(vf_x, vf_y, vf_w, vf_h, cols, rows, col, row);
            if (x - ex).abs() <= eps
                && (y - ey).abs() <= eps
                && (w - ew).abs() <= eps
                && (h - eh).abs() <= eps
            {
                return Some((col, row));
            }
        }
    }
    None
}

fn log_frontmost() {
    if let Some(w) = mac_winops::frontmost_window() {
        info!("focus-nav: now on window title='{}' pid={}", w.title, w.pid);
    } else {
        info!("focus-nav: now on window title=<none>");
    }
}

pub fn run_focus_nav_test(timeout_ms: u64, with_logs: bool) -> Result<()> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let title_tl = format!("hotki smoketest: focus-tl {}-{}", std::process::id(), now);
    let title_tr = format!("hotki smoketest: focus-tr {}-{}", std::process::id(), now);
    let title_bl = format!("hotki smoketest: focus-bl {}-{}", std::process::id(), now);
    let title_br = format!("hotki smoketest: focus-br {}-{}", std::process::id(), now);

    // Minimal config: direct global bindings for focus directions to avoid HUD submenu latency
    let ron_config = format!(
        "(\n    keys: [\n        (\"ctrl+alt+h\", \"left\", focus(left), (global: true, hide: true)),\n        (\"ctrl+alt+l\", \"right\", focus(right), (global: true, hide: true)),\n        (\"ctrl+alt+k\", \"up\", focus(up), (global: true, hide: true)),\n        (\"ctrl+alt+j\", \"down\", focus(down), (global: true, hide: true)),\n        (\"ctrl+alt+1\", \"tl\", raise(title: \"{}\"), (global: true, hide: true)),\n        (\"ctrl+alt+2\", \"tr\", raise(title: \"{}\"), (global: true, hide: true)),\n        (\"ctrl+alt+3\", \"bl\", raise(title: \"{}\"), (global: true, hide: true)),\n        (\"ctrl+alt+4\", \"br\", raise(title: \"{}\"), (global: true, hide: true)),\n    ],\n    style: (hud: (mode: hide))\n)\n",
        title_tl, title_tr, title_bl, title_br
    );

    let config = TestConfig::new(timeout_ms)
        .with_logs(with_logs)
        .with_temp_config(ron_config.to_string());

    TestRunner::new("focus_nav_test", config)
        .with_setup(|ctx| {
            ctx.launch_hotki()?;
            // Initialize RPC driver and gate on one of the direct bindings to avoid HUD waits
            if let Some(sess) = ctx.session.as_ref() {
                let sock = sess.socket_path().to_string();
                let start = std::time::Instant::now();
                let mut inited = crate::server_drive::init(&sock);
                while !inited && start.elapsed() < std::time::Duration::from_millis(3000) {
                    std::thread::sleep(std::time::Duration::from_millis(50));
                    inited = crate::server_drive::init(&sock);
                }
                let _ = crate::server_drive::wait_for_ident(
                    "ctrl+alt+h",
                    crate::config::BINDING_GATE_DEFAULT_MS,
                );
            }
            Ok(())
        })
        .with_execute(move |_ctx| {
            // Spawn helpers with longer lifetimes to accommodate placement/tests
            let helper_time = timeout_ms.saturating_add(4000);
            let tl = HelperWindowBuilder::new(&title_tl)
                .with_time_ms(helper_time)
                .with_grid(2, 2, 0, 0)
                .with_label_text("TL")
                .spawn()?;
            let tr = HelperWindowBuilder::new(&title_tr)
                .with_time_ms(helper_time)
                .with_grid(2, 2, 1, 0)
                .with_label_text("TR")
                .spawn()?;
            let bl = HelperWindowBuilder::new(&title_bl)
                .with_time_ms(helper_time)
                .with_grid(2, 2, 0, 1)
                .with_label_text("BL")
                .spawn()?;
            let br = HelperWindowBuilder::new(&title_br)
                .with_time_ms(helper_time)
                .with_grid(2, 2, 1, 1)
                .with_label_text("BR")
                .spawn()?;

            // Ensure visibility before arranging
            if !wait_for_windows_visible(
                &[
                    (tl.pid, &title_tl),
                    (tr.pid, &title_tr),
                    (bl.pid, &title_bl),
                    (br.pid, &title_br),
                ],
                config::WAIT_BOTH_WINDOWS_MS,
            ) {
                return Err(Error::FocusNotObserved {
                    timeout_ms: config::WAIT_BOTH_WINDOWS_MS,
                    expected: "helpers not visible (4)".into(),
                });
            }

            // Helpers self-place into 2x2 via mac-winops; no server placement required

            // Establish initial focus quickly via direct raise binding
            send_key("ctrl+alt+1");
            if !wait_for_frontmost_title(&title_tl, config::FOCUS_NAV_STEP_TIMEOUT_MS) {
                return Err(Error::FocusNotObserved {
                    timeout_ms,
                    expected: title_tl.clone(),
                });
            }
            // Confirm explicitly: TL has focus at start
            info!("focus-nav: START — expecting TL focused");
            log_frontmost();
            // Check TL is at cell (0,0) within epsilon
            let front = mac_winops::frontmost_window()
                .ok_or_else(|| Error::InvalidState("No frontmost CG window".into()))?;
            let ((x, y), (w, h)) = mac_winops::ax_window_frame(front.pid, &front.title)
                .ok_or_else(|| {
                    Error::InvalidState("AX frame for frontmost not available".into())
                })?;
            let (vf_x, vf_y, vf_w, vf_h) = visible_frame_containing_point(x, y)
                .ok_or_else(|| Error::InvalidState("No visibleFrame for frontmost".into()))?;
            if let Some((cx, cy)) =
                find_cell_for_frame(vf_x, vf_y, vf_w, vf_h, 2, 2, x, y, w, h, EPS)
            {
                info!(
                    "focus-nav: TL cell coords=({}, {}) — expecting (0, 0)",
                    cx, cy
                );
                if !(cx == 0 && cy == 0) {
                    return Err(Error::InvalidState(format!(
                        "TL not at (0,0): got ({}, {})",
                        cx, cy
                    )));
                }
            } else {
                return Err(Error::InvalidState(
                    "Could not resolve TL cell coords".into(),
                ));
            }
            assert_frontmost_cell(&title_tl, 0, 0)?;

            // Helper to drive focus(dir) via direct global bindings
            let drive = |dir: &str| {
                match dir {
                    "h" => send_key("ctrl+alt+h"),
                    "l" => send_key("ctrl+alt+l"),
                    "k" => send_key("ctrl+alt+k"),
                    "j" => send_key("ctrl+alt+j"),
                    _ => {}
                }
                log_frontmost();
            };

            // TL -> TR
            // Verify source before move
            assert_frontmost_cell(&title_tl, 0, 0)?;
            drive("l"); // RIGHT
            if !wait_for_frontmost_title(&title_tr, config::FOCUS_NAV_STEP_TIMEOUT_MS) {
                return Err(Error::FocusNotObserved {
                    timeout_ms,
                    expected: title_tr.clone(),
                });
            }
            assert_frontmost_cell(&title_tr, 1, 0)?;
            // TR -> BR
            // Verify source before move
            assert_frontmost_cell(&title_tr, 1, 0)?;
            drive("j"); // DOWN
            if !wait_for_frontmost_title(&title_br, config::FOCUS_NAV_STEP_TIMEOUT_MS) {
                return Err(Error::FocusNotObserved {
                    timeout_ms,
                    expected: title_br.clone(),
                });
            }
            assert_frontmost_cell(&title_br, 1, 1)?;
            // BR -> BL
            // Verify source before move
            assert_frontmost_cell(&title_br, 1, 1)?;
            drive("h"); // LEFT
            if !wait_for_frontmost_title(&title_bl, config::FOCUS_NAV_STEP_TIMEOUT_MS) {
                return Err(Error::FocusNotObserved {
                    timeout_ms,
                    expected: title_bl.clone(),
                });
            }
            assert_frontmost_cell(&title_bl, 0, 1)?;
            // BL -> TL
            // Verify source before move
            assert_frontmost_cell(&title_bl, 0, 1)?;
            drive("k"); // UP
            if !wait_for_frontmost_title(&title_tl, config::FOCUS_NAV_STEP_TIMEOUT_MS) {
                return Err(Error::FocusNotObserved {
                    timeout_ms,
                    expected: title_tl.clone(),
                });
            }
            // Final explicit confirmation: back at TL and at (0,0)
            log_frontmost();
            let front = mac_winops::frontmost_window()
                .ok_or_else(|| Error::InvalidState("No frontmost CG window".into()))?;
            let ((x, y), (w, h)) = mac_winops::ax_window_frame(front.pid, &front.title)
                .ok_or_else(|| {
                    Error::InvalidState("AX frame for frontmost not available".into())
                })?;
            let (vf_x, vf_y, vf_w, vf_h) = visible_frame_containing_point(x, y)
                .ok_or_else(|| Error::InvalidState("No visibleFrame for frontmost".into()))?;
            if let Some((cx, cy)) =
                find_cell_for_frame(vf_x, vf_y, vf_w, vf_h, 2, 2, x, y, w, h, EPS)
            {
                info!(
                    "focus-nav: END — TL cell coords=({}, {}) — expecting (0, 0)",
                    cx, cy
                );
                if !(cx == 0 && cy == 0) {
                    return Err(Error::InvalidState(format!(
                        "END TL not at (0,0): got ({}, {})",
                        cx, cy
                    )));
                }
            } else {
                return Err(Error::InvalidState(
                    "Could not resolve END TL cell coords".into(),
                ));
            }
            assert_frontmost_cell(&title_tl, 0, 0)?;
            Ok(())
        })
        .run()
}

// No HUD fallback: rely on CG frontmost title for focus verification to reduce IPC noise.
