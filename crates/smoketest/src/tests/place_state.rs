//! Placement normalization smoketests: exercise minimized/zoomed pre-states.

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use objc2_foundation::MainThreadMarker;

use super::helpers::spawn_helper_with_options;
use crate::{
    config,
    error::{Error, Result},
    server_drive,
    test_runner::{TestConfig, TestRunner},
    ui_interaction::send_key,
};

fn visible_frame_containing_point(x: f64, y: f64) -> Option<(f64, f64, f64, f64)> {
    let mtm = MainThreadMarker::new()?;
    for s in objc2_app_kit::NSScreen::screens(mtm).iter() {
        let fr = s.visibleFrame();
        let sx = fr.origin.x;
        let sy = fr.origin.y;
        let sw = fr.size.width;
        let sh = fr.size.height;
        if x >= sx && x <= sx + sw && y >= sy && y <= sy + sh {
            return Some((sx, sy, sw, sh));
        }
    }
    if let Some(scr) = objc2_app_kit::NSScreen::mainScreen(mtm) {
        let r = scr.visibleFrame();
        return Some((r.origin.x, r.origin.y, r.size.width, r.size.height));
    }
    if let Some(s) = objc2_app_kit::NSScreen::screens(mtm).iter().next() {
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

fn run_place_with_state(
    timeout_ms: u64,
    with_logs: bool,
    start_minimized: bool,
    start_zoomed: bool,
    label: String,
) -> Result<()> {
    let cols = crate::config::PLACE_COLS;
    let rows = crate::config::PLACE_ROWS;
    let now_pre = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let helper_title = format!(
        "hotki smoketest: place-state {}-{}",
        std::process::id(),
        now_pre
    );
    // Minimal config: raise + place (0,0) on key '1'
    let ron_config = format!(
        "(\n    keys: [\n        (\"g\", \"raise\", raise(title: \"{}\"), (noexit: true)),\n        (\"1\", \"(0,0)\", place(grid({}, {}), at(0, 0))),\n    ],\n    style: (hud: (mode: hide)),\n    server: (exit_if_no_clients: true),\n)\n",
        helper_title, cols, rows
    );
    let config = TestConfig::new(timeout_ms)
        .with_logs(with_logs)
        .with_temp_config(ron_config);

    TestRunner::new("place_state", config)
        .with_setup(|ctx| {
            ctx.launch_hotki()?;
            if let Some(sess) = ctx.session.as_ref() {
                let sock = sess.socket_path().to_string();
                let start = std::time::Instant::now();
                let mut inited = server_drive::init(&sock);
                while !inited && start.elapsed() < std::time::Duration::from_millis(3000) {
                    std::thread::sleep(std::time::Duration::from_millis(50));
                    inited = server_drive::init(&sock);
                }
                let _ = server_drive::wait_for_ident("g", crate::config::BINDING_GATE_DEFAULT_MS);
                let _ = server_drive::wait_for_ident("1", crate::config::BINDING_GATE_DEFAULT_MS);
            }
            Ok(())
        })
        .with_execute(move |ctx| {
            let title = helper_title.clone();
            let helper_time = ctx
                .config
                .timeout_ms
                .saturating_add(config::HELPER_WINDOW_EXTRA_TIME_MS);
            let mut helper = spawn_helper_with_options(
                title.clone(),
                helper_time,
                std::cmp::min(ctx.config.timeout_ms, config::HIDE_FIRST_WINDOW_MAX_MS),
                config::PLACE_POLL_MS,
                &label,
                start_minimized,
                start_zoomed,
            )?;

            // Ensure frontmost by driving the raise binding a few times and waiting
            for _ in 0..3 {
                send_key("g");
                if super::helpers::wait_for_frontmost_title(&title, config::WAIT_FIRST_WINDOW_MS) {
                    break;
                }
            }

            let (vf_x, vf_y, vf_w, vf_h) = if let Some((px, py)) =
                mac_winops::ax_window_position(helper.pid, &title)
                && let Some(vf) = visible_frame_containing_point(px, py)
            {
                vf
            } else if let Some(mtm) = MainThreadMarker::new()
                && let Some(scr) = objc2_app_kit::NSScreen::mainScreen(mtm)
            {
                let r = scr.visibleFrame();
                (r.origin.x, r.origin.y, r.size.width, r.size.height)
            } else {
                return Err(Error::InvalidState(
                    "Failed to resolve screen visibleFrame".into(),
                ));
            };

            // Expected cell rect for (0,0)
            let (ex, ey, ew, eh) = cell_rect(vf_x, vf_y, vf_w, vf_h, cols, rows, 0, 0);
            send_key("1");
            let ok = wait_for_expected_frame(
                helper.pid,
                &title,
                (ex, ey, ew, eh),
                config::PLACE_EPS,
                config::PLACE_STEP_TIMEOUT_MS,
                config::PLACE_POLL_MS,
            );
            if !ok {
                return Err(Error::InvalidState("placement verification failed".into()));
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

pub fn run_place_minimized_test(timeout_ms: u64, with_logs: bool) -> Result<()> {
    run_place_with_state(timeout_ms, with_logs, true, false, "M".to_string())
}

pub fn run_place_zoomed_test(timeout_ms: u64, with_logs: bool) -> Result<()> {
    run_place_with_state(timeout_ms, with_logs, false, true, "Z".to_string())
}
