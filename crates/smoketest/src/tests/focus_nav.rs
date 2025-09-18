//! Focus navigation smoketest using the new `focus(dir)` action.

use std::{thread, time::Duration};

use mac_winops;
// no direct std::time imports needed beyond internal helpers
use tracing::info;

use super::fixtures::{self, Rect, wait_for_windows_visible};
use crate::{
    config,
    error::{Error, Result},
    helper_window::{
        FRONTMOST_IGNORE_TITLES, HelperWindowBuilder, frontmost_app_window,
        wait_for_frontmost_title,
    },
    server_drive,
    test_runner::{TestConfig, TestRunner},
    ui_interaction::send_key,
    world,
};

// Placement handled via server by driving config bindings (no direct WinOps here).

/// Tolerance when comparing expected vs observed frames.
const EPS: f64 = 2.0;

/// Resolve the visible frame for the screen containing the frontmost window.
fn current_frontmost_vf() -> Result<Rect> {
    let front = frontmost_app_window(FRONTMOST_IGNORE_TITLES)
        .ok_or_else(|| Error::InvalidState("No frontmost app window".into()))?;
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
    if let Some(w) = frontmost_app_window(FRONTMOST_IGNORE_TITLES) {
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
            let helper_time = timeout_ms.saturating_add(4000);
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
            let tl = HelperWindowBuilder::new(&title_tl)
                .with_time_ms(helper_time)
                .with_grid(2, 2, 0, 0)
                .with_label_text("TL")
                .spawn()?;

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
            info!("focus-nav: helpers visible");

            for ident in [
                "ctrl+alt+1",
                "ctrl+alt+h",
                "ctrl+alt+l",
                "ctrl+alt+k",
                "ctrl+alt+j",
            ] {
                server_drive::wait_for_ident(ident, config::BINDING_GATES.default_ms * 2)?;
            }

            let helpers = [
                (&title_tl, (0u32, 0u32)),
                (&title_tr, (1, 0)),
                (&title_br, (1, 1)),
                (&title_bl, (0, 1)),
            ];

            let mut placements_ok = false;
            for _ in 0..50 {
                let snapshot = world::list_windows()?;
                let mut all_ok = true;
                for (title, expected) in helpers.iter() {
                    let Some(win) = snapshot.iter().find(|w| w.title == **title) else {
                        all_ok = false;
                        break;
                    };
                    let Some(pos) = win.pos else {
                        all_ok = false;
                        break;
                    };
                    let rect = Rect::new(
                        pos.x.into(),
                        pos.y.into(),
                        pos.width.into(),
                        pos.height.into(),
                    );
                    let Some(vf) = fixtures::visible_frame_containing_point(rect.x, rect.y) else {
                        all_ok = false;
                        break;
                    };
                    let Some((col, row)) = find_cell_for_frame(vf, 2, 2, rect, EPS) else {
                        all_ok = false;
                        break;
                    };
                    if (col, row) != *expected {
                        all_ok = false;
                        break;
                    }
                }
                if all_ok {
                    placements_ok = true;
                    break;
                }
                thread::sleep(Duration::from_millis(100));
            }
            if !placements_ok {
                return Err(Error::InvalidState(
                    "Helper windows did not settle into expected grid".to_string(),
                ));
            }

            let titles = [&title_tl, &title_tr, &title_br, &title_bl];
            let coords = [(0u32, 0u32), (1, 0), (1, 1), (0, 1)];
            let dirs = ["l", "j", "h", "k"];

            let mut front = frontmost_app_window(FRONTMOST_IGNORE_TITLES)
                .ok_or_else(|| Error::InvalidState("No frontmost app window".into()))?;
            let mut start_idx = titles
                .iter()
                .position(|t| t.as_str() == front.title)
                .or_else(|| {
                    if mac_winops::ensure_frontmost_by_title(
                        tl.pid,
                        &title_tl,
                        3,
                        config::INPUT_DELAYS.retry_delay_ms,
                    ) && wait_for_frontmost_title(&title_tl, 500)
                    {
                        Some(0)
                    } else {
                        None
                    }
                })
                .ok_or_else(|| {
                    Error::InvalidState("Frontmost helper window not detected".into())
                })?;

            if titles[start_idx].as_str() != front.title {
                front = frontmost_app_window(FRONTMOST_IGNORE_TITLES)
                    .ok_or_else(|| Error::InvalidState("No frontmost app window".into()))?;
                start_idx = titles
                    .iter()
                    .position(|t| t.as_str() == front.title)
                    .ok_or_else(|| {
                        Error::InvalidState("Frontmost helper window not detected".into())
                    })?;
            }

            let mut rotated_titles = Vec::with_capacity(4);
            let mut rotated_coords = Vec::with_capacity(4);
            let mut rotated_dirs = Vec::with_capacity(4);
            for step in 0..4 {
                let idx = (start_idx + step) % 4;
                rotated_titles.push(titles[idx]);
                rotated_coords.push(coords[idx]);
                rotated_dirs.push(dirs[idx]);
            }

            info!(
                start = rotated_titles[0].as_str(),
                "focus-nav: starting focus cycle"
            );

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

            for step in 0..rotated_dirs.len() {
                let current_title = rotated_titles[step];
                let (cx, cy) = rotated_coords[step];
                fixtures::assert_frontmost_cell(
                    current_title,
                    current_frontmost_vf()?,
                    2,
                    2,
                    cx,
                    cy,
                    EPS,
                )?;
                drive(rotated_dirs[step])?;
                let next_title = rotated_titles[(step + 1) % rotated_titles.len()];
                if !wait_for_frontmost_title(next_title, config::FOCUS_NAV.step_timeout_ms) {
                    return Err(Error::FocusNotObserved {
                        timeout_ms,
                        expected: next_title.clone(),
                    });
                }
                let (nx, ny) = rotated_coords[(step + 1) % rotated_coords.len()];
                fixtures::assert_frontmost_cell(
                    next_title,
                    current_frontmost_vf()?,
                    2,
                    2,
                    nx,
                    ny,
                    EPS,
                )?;
            }

            log_frontmost();

            Ok(())
        })
        .run()
}

// No HUD fallback: rely on CG frontmost title for focus verification to reduce IPC noise.
