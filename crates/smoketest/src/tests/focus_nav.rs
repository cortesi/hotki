//! Focus navigation smoketest using the new `focus(dir)` action.

// no direct std::time imports needed beyond internal helpers

use tracing::info;

use super::fixtures::{self, Rect, wait_for_windows_visible};
use crate::{
    config,
    error::{Error, Result},
    helper_window::{HelperWindowBuilder, wait_for_frontmost_title},
    server_drive,
    test_runner::{TestConfig, TestRunner},
    ui_interaction::send_key,
};

// Placement handled via server by driving config bindings (no direct WinOps here).

/// Tolerance when comparing expected vs observed frames.
const EPS: f64 = 2.0;

/// Resolve the visible frame for the screen containing the frontmost window.
fn current_frontmost_vf() -> Result<Rect> {
    let front = mac_winops::frontmost_window()
        .ok_or_else(|| Error::InvalidState("No frontmost CG window".into()))?;
    let ((x, y), _) = mac_winops::ax_window_frame(front.pid, &front.title)
        .ok_or_else(|| Error::InvalidState("AX frame for frontmost not available".into()))?;
    fixtures::visible_frame_containing_point(x, y)
        .ok_or_else(|| Error::InvalidState("No visibleFrame for frontmost".into()))
}

/// Find the grid cell index for a given frame within the visible frame.
fn find_cell_for_frame(
    vf: Rect,
    cols: u32,
    rows: u32,
    frame: Rect,
    eps: f64,
) -> Option<(u32, u32)> {
    for row in 0..rows {
        for col in 0..cols {
            let expected = fixtures::cell_rect(vf, cols, rows, col, row);
            if frame.approx_eq(&expected, eps) {
                return Some((col, row));
            }
        }
    }
    None
}

/// Log the frontmost window title and pid for debugging.
fn log_frontmost() {
    if let Some(w) = mac_winops::frontmost_window() {
        info!("focus-nav: now on window title='{}' pid={}", w.title, w.pid);
    } else {
        info!("focus-nav: now on window title=<none>");
    }
}

/// Run focus navigation test across a 2x2 grid of helpers.
pub fn run_focus_nav_test(timeout_ms: u64, with_logs: bool) -> Result<()> {
    let title_tl = config::test_title("focus-tl");
    let title_tr = config::test_title("focus-tr");
    let title_bl = config::test_title("focus-bl");
    let title_br = config::test_title("focus-br");

    // Minimal config: direct global bindings for focus directions to avoid HUD submenu latency
    let ron_config = format!(
        "(\n    keys: [\n        (\"ctrl+alt+h\", \"left\", focus(left), (global: true, hide: true)),\n        (\"ctrl+alt+l\", \"right\", focus(right), (global: true, hide: true)),\n        (\"ctrl+alt+k\", \"up\", focus(up), (global: true, hide: true)),\n        (\"ctrl+alt+j\", \"down\", focus(down), (global: true, hide: true)),\n        (\"ctrl+alt+1\", \"tl\", raise(title: \"{}\"), (global: true, hide: true)),\n        (\"ctrl+alt+2\", \"tr\", raise(title: \"{}\"), (global: true, hide: true)),\n        (\"ctrl+alt+3\", \"bl\", raise(title: \"{}\"), (global: true, hide: true)),\n        (\"ctrl+alt+4\", \"br\", raise(title: \"{}\"), (global: true, hide: true)),\n    ],\n    style: (hud: (mode: hide))\n)\n",
        title_tl, title_tr, title_bl, title_br
    );

    let config = TestConfig::new(timeout_ms)
        .with_logs(with_logs)
        .with_temp_config(ron_config);

    TestRunner::new("focus_nav_test", config)
        .with_setup(|ctx| {
            ctx.launch_hotki()?;
            ctx.ensure_rpc_ready(&[
                "ctrl+alt+1",
                "ctrl+alt+2",
                "ctrl+alt+3",
                "ctrl+alt+4",
                "ctrl+alt+h",
                "ctrl+alt+l",
                "ctrl+alt+k",
                "ctrl+alt+j",
            ])?;
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
                config::WAITS.both_windows_ms,
            ) {
                return Err(Error::FocusNotObserved {
                    timeout_ms: config::WAITS.both_windows_ms,
                    expected: "helpers not visible (4)".into(),
                });
            }

            for ident in [
                "ctrl+alt+1",
                "ctrl+alt+h",
                "ctrl+alt+l",
                "ctrl+alt+k",
                "ctrl+alt+j",
            ] {
                server_drive::wait_for_ident(ident, config::BINDING_GATES.default_ms * 2)?;
            }

            // Helpers self-place into 2x2 via mac-winops; no server placement required

            // Establish initial focus quickly via direct raise binding
            send_key("ctrl+alt+1")?;
            if !wait_for_frontmost_title(&title_tl, config::FOCUS_NAV.step_timeout_ms) {
                return Err(Error::FocusNotObserved {
                    timeout_ms,
                    expected: title_tl,
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
            let vf = fixtures::visible_frame_containing_point(x, y)
                .ok_or_else(|| Error::InvalidState("No visibleFrame for frontmost".into()))?;
            let frame = Rect::new(x, y, w, h);
            if let Some((cx, cy)) = find_cell_for_frame(vf, 2, 2, frame, EPS) {
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
            fixtures::assert_frontmost_cell(&title_tl, current_frontmost_vf()?, 2, 2, 0, 0, EPS)?;

            // Helper to drive focus(dir) via direct global bindings
            let drive = |dir: &str| -> Result<()> {
                match dir {
                    "h" => send_key("ctrl+alt+h")?,
                    "l" => send_key("ctrl+alt+l")?,
                    "k" => send_key("ctrl+alt+k")?,
                    "j" => send_key("ctrl+alt+j")?,
                    _ => {}
                }
                log_frontmost();
                Ok(())
            };

            // TL -> TR
            // Verify source before move
            fixtures::assert_frontmost_cell(&title_tl, current_frontmost_vf()?, 2, 2, 0, 0, EPS)?;
            drive("l")?; // RIGHT
            if !wait_for_frontmost_title(&title_tr, config::FOCUS_NAV.step_timeout_ms) {
                return Err(Error::FocusNotObserved {
                    timeout_ms,
                    expected: title_tr,
                });
            }
            fixtures::assert_frontmost_cell(&title_tr, current_frontmost_vf()?, 2, 2, 1, 0, EPS)?;
            // TR -> BR
            // Verify source before move
            fixtures::assert_frontmost_cell(&title_tr, current_frontmost_vf()?, 2, 2, 1, 0, EPS)?;
            drive("j")?; // DOWN
            if !wait_for_frontmost_title(&title_br, config::FOCUS_NAV.step_timeout_ms) {
                return Err(Error::FocusNotObserved {
                    timeout_ms,
                    expected: title_br,
                });
            }
            fixtures::assert_frontmost_cell(&title_br, current_frontmost_vf()?, 2, 2, 1, 1, EPS)?;
            // BR -> BL
            // Verify source before move
            fixtures::assert_frontmost_cell(&title_br, current_frontmost_vf()?, 2, 2, 1, 1, EPS)?;
            drive("h")?; // LEFT
            if !wait_for_frontmost_title(&title_bl, config::FOCUS_NAV.step_timeout_ms) {
                return Err(Error::FocusNotObserved {
                    timeout_ms,
                    expected: title_bl,
                });
            }
            fixtures::assert_frontmost_cell(&title_bl, current_frontmost_vf()?, 2, 2, 0, 1, EPS)?;
            // BL -> TL
            // Verify source before move
            fixtures::assert_frontmost_cell(&title_bl, current_frontmost_vf()?, 2, 2, 0, 1, EPS)?;
            drive("k")?; // UP
            if !wait_for_frontmost_title(&title_tl, config::FOCUS_NAV.step_timeout_ms) {
                return Err(Error::FocusNotObserved {
                    timeout_ms,
                    expected: title_tl,
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
            let vf = fixtures::visible_frame_containing_point(x, y)
                .ok_or_else(|| Error::InvalidState("No visibleFrame for frontmost".into()))?;
            let frame = Rect::new(x, y, w, h);
            if let Some((cx, cy)) = find_cell_for_frame(vf, 2, 2, frame, EPS) {
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
            fixtures::assert_frontmost_cell(&title_tl, current_frontmost_vf()?, 2, 2, 0, 0, EPS)?;
            Ok(())
        })
        .run()
}

// No HUD fallback: rely on CG frontmost title for focus verification to reduce IPC noise.
