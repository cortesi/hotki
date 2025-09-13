//! Flexible placement smoketest used to exercise Stage-3/8 behaviors.
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use mac_winops::PlaceAttemptOptions;
use objc2_app_kit::NSScreen;
use objc2_foundation::MainThreadMarker;

use crate::{
    config,
    error::{Error, Result},
    tests::helpers::{spawn_helper_visible, wait_for_frontmost_title},
};

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

fn resolve_vf_for_window(
    pid: i32,
    title: &str,
    timeout_ms: u64,
    poll_ms: u64,
) -> Option<(f64, f64, f64, f64)> {
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    while Instant::now() < deadline {
        if let Some((px, py)) = mac_winops::ax_window_position(pid, title)
            && let Some(vf) = visible_frame_containing_point(px, py)
        {
            return Some(vf);
        }
        std::thread::sleep(Duration::from_millis(poll_ms));
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

pub fn run_place_flex(
    cols: u32,
    rows: u32,
    col: u32,
    row: u32,
    force_size_pos: bool,
    pos_first_only: bool,
    force_shrink_move_grow: bool,
) -> Result<()> {
    // Create unique helper title
    let now_pre = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let title = format!(
        "hotki smoketest: place-flex {}-{}",
        std::process::id(),
        now_pre
    );

    // Spawn helper and wait until visible
    let lifetime = config::DEFAULT_TIMEOUT_MS + config::HELPER_WINDOW_EXTRA_TIME_MS;
    let mut helper = spawn_helper_visible(
        title.clone(),
        lifetime,
        std::cmp::min(config::DEFAULT_TIMEOUT_MS, config::HIDE_FIRST_WINDOW_MAX_MS),
        config::PLACE_POLL_MS,
        "FLEX",
    )?;

    // Bring to front to ensure mac-winops targets the correct focused window
    crate::tests::helpers::ensure_frontmost(helper.pid, &title, 3, 50);
    let _ = wait_for_frontmost_title(&title, config::WAIT_FIRST_WINDOW_MS);

    // Compute expected rect from screen VF containing current AX position
    let (vf_x, vf_y, vf_w, vf_h) = resolve_vf_for_window(
        helper.pid,
        &title,
        config::DEFAULT_TIMEOUT_MS,
        config::PLACE_POLL_MS,
    )
    .ok_or_else(|| Error::InvalidState("Failed to resolve screen visibleFrame".into()))?;
    let (ex, ey, ew, eh) = cell_rect(vf_x, vf_y, vf_w, vf_h, cols, rows, col, row);

    // Build attempt options for placement
    let opts = PlaceAttemptOptions {
        force_second_attempt: force_size_pos || force_shrink_move_grow,
        pos_first_only,
        force_shrink_move_grow,
    };

    // Call mac-winops directly to place the focused window
    mac_winops::place_grid_focused_opts(helper.pid, cols, rows, col, row, opts)
        .map_err(|e| Error::InvalidState(format!("place_grid_focused failed: {}", e)))?;

    // Verify expected frame
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
                "place-flex mismatch (expected x={:.1} y={:.1} w={:.1} h={:.1}; actual x={:.1} y={:.1} w={:.1} h={:.1})",
                ex, ey, ew, eh, ax, ay, aw, ah
            ),
            None => format!(
                "place-flex mismatch (expected x={:.1} y={:.1} w={:.1} h={:.1}; actual frame unavailable)",
                ex, ey, ew, eh
            ),
        }));
    }

    let _ = helper.kill_and_wait();
    Ok(())
}
