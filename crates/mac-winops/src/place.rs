use std::cmp::min;

use objc2_foundation::MainThreadMarker;
use tracing::debug;

use crate::{
    Error, Result, WindowId,
    ax::{
        ax_bool, ax_check, ax_get_point, ax_get_size, ax_set_bool, ax_set_point, ax_set_size,
        ax_window_for_id, cfstr,
    },
    geom::{self, CGPoint, CGSize, Rect},
    screen_util::visible_frame_containing_point,
};

/// Epsilon tolerance (in points) used to verify post‑placement position and size.
const VERIFY_EPS: f64 = 2.0;

const POLL_SLEEP_MS: u64 = 25;
const POLL_TOTAL_MS: u64 = 400;

// Stage 2: settle/polling parameters for apply_and_wait
const APPLY_STUTTER_MS: u64 = 2; // tiny delay between A and B sets
const SETTLE_SLEEP_MS: u64 = 20; // poll cadence while waiting to settle
const SETTLE_TOTAL_MS: u64 = 250; // max settle time per attempt

#[inline]
fn sleep_ms(ms: u64) {
    use std::{thread::sleep, time::Duration};
    sleep(Duration::from_millis(ms));
}

/// Best‑effort window state normalization prior to placement:
/// - Bail if system Full Screen is active.
/// - If minimized/zoomed, turn off and wait briefly.
/// - Try to raise the window (ignore unsupported/failed).
fn normalize_before_move(win: &crate::AXElem, pid: i32, id_opt: Option<WindowId>) -> Result<()> {
    // 1) Bail on macOS Full Screen (separate Space)
    match ax_bool(win.as_ptr(), cfstr("AXFullScreen")) {
        Ok(Some(true)) => {
            debug!("normalize: fullscreen=true -> bail");
            return Err(Error::FullscreenActive);
        }
        Ok(Some(false)) => {
            debug!("normalize: fullscreen=false");
        }
        _ => {
            // Attribute unsupported/missing — ignore silently.
        }
    }

    // 2) If minimized, unminimize and wait
    match ax_bool(win.as_ptr(), cfstr("AXMinimized")) {
        Ok(Some(true)) => {
            debug!("normalize: AXMinimized=true -> set false");
            let _ = ax_set_bool(win.as_ptr(), cfstr("AXMinimized"), false);
            let mut waited = 0u64;
            while waited <= POLL_TOTAL_MS {
                if let Ok(Some(false)) = ax_bool(win.as_ptr(), cfstr("AXMinimized")) {
                    break;
                }
                sleep_ms(POLL_SLEEP_MS);
                waited = waited.saturating_add(POLL_SLEEP_MS);
            }
        }
        Ok(Some(false)) => {}
        _ => {}
    }

    // 3) If zoomed, unzoom and wait briefly
    match ax_bool(win.as_ptr(), cfstr("AXZoomed")) {
        Ok(Some(true)) => {
            debug!("normalize: AXZoomed=true -> set false");
            let _ = ax_set_bool(win.as_ptr(), cfstr("AXZoomed"), false);
            let mut waited = 0u64;
            while waited <= POLL_TOTAL_MS {
                if let Ok(Some(false)) = ax_bool(win.as_ptr(), cfstr("AXZoomed")) {
                    break;
                }
                sleep_ms(POLL_SLEEP_MS);
                waited = waited.saturating_add(POLL_SLEEP_MS);
            }
        }
        Ok(Some(false)) => {}
        _ => {}
    }

    // 4) Best‑effort raise: prefer our AX window; for known id, also use raise helper.
    // Try direct AXRaise on the window first.
    unsafe {
        #[allow(improper_ctypes)]
        unsafe extern "C" {
            fn AXUIElementPerformAction(
                element: *mut core::ffi::c_void,
                action: core_foundation::string::CFStringRef,
            ) -> i32;
        }
        let _ = AXUIElementPerformAction(win.as_ptr(), cfstr("AXRaise"));
    }
    if let Some(id) = id_opt {
        let _ = crate::raise::raise_window(pid, id);
    }
    Ok(())
}

#[inline]
fn rect_from(x: f64, y: f64, w: f64, h: f64) -> Rect {
    Rect { x, y, w, h }
}

#[inline]
fn diffs(a: &Rect, b: &Rect) -> (f64, f64, f64, f64) {
    (
        (a.x - b.x).abs(),
        (a.y - b.y).abs(),
        (a.w - b.w).abs(),
        (a.h - b.h).abs(),
    )
}

#[inline]
fn within_eps(d: (f64, f64, f64, f64), eps: f64) -> bool {
    d.0 <= eps && d.1 <= eps && d.2 <= eps && d.3 <= eps
}

#[inline]
fn clamp_flags(got: &Rect, vf: &Rect, eps: f64) -> String {
    let mut flags: Vec<&str> = Vec::new();
    if geom::approx_eq(got.left(), vf.left(), eps) {
        flags.push("left");
    }
    if geom::approx_eq(got.right(), vf.right(), eps) {
        flags.push("right");
    }
    if geom::approx_eq(got.bottom(), vf.bottom(), eps) {
        flags.push("bottom");
    }
    if geom::approx_eq(got.top(), vf.top(), eps) {
        flags.push("top");
    }
    if flags.is_empty() {
        "none".into()
    } else {
        flags.join(",")
    }
}

#[inline]
fn log_summary(order: &str, attempt: u32, eps: f64, d: (f64, f64, f64, f64)) {
    debug!(
        "summary: order={} attempt={} eps={:.1} dx={:.2} dy={:.2} dw={:.2} dh={:.2}",
        order, attempt, eps, d.0, d.1, d.2, d.3
    );
}

#[inline]
fn now_ms(start: std::time::Instant) -> u64 {
    start.elapsed().as_millis() as u64
}

/// Apply target position/size in a given order and poll until the window frame
/// settles within `eps`, or until `SETTLE_TOTAL_MS` elapses. Returns the last
/// observed rect and the measured settle time in milliseconds.
fn apply_and_wait(
    op_label: &str,
    win: &crate::AXElem,
    attr_pos: core_foundation::string::CFStringRef,
    attr_size: core_foundation::string::CFStringRef,
    target: &Rect,
    pos_first: bool,
    eps: f64,
) -> Result<(Rect, u64)> {
    let start = std::time::Instant::now();

    // 1) Apply in requested order with a tiny stutter between A and B.
    if pos_first {
        debug!(
            "WinOps: {} set pos -> ({:.1},{:.1})",
            op_label, target.x, target.y
        );
        ax_set_point(
            win.as_ptr(),
            attr_pos,
            CGPoint {
                x: target.x,
                y: target.y,
            },
        )?;
        sleep_ms(APPLY_STUTTER_MS);
        debug!(
            "WinOps: {} set size -> ({:.1},{:.1})",
            op_label, target.w, target.h
        );
        ax_set_size(
            win.as_ptr(),
            attr_size,
            CGSize {
                width: target.w,
                height: target.h,
            },
        )?;
    } else {
        debug!(
            "WinOps: {} set size -> ({:.1},{:.1})",
            op_label, target.w, target.h
        );
        ax_set_size(
            win.as_ptr(),
            attr_size,
            CGSize {
                width: target.w,
                height: target.h,
            },
        )?;
        sleep_ms(APPLY_STUTTER_MS);
        debug!(
            "WinOps: {} set pos -> ({:.1},{:.1})",
            op_label, target.x, target.y
        );
        ax_set_point(
            win.as_ptr(),
            attr_pos,
            CGPoint {
                x: target.x,
                y: target.y,
            },
        )?;
    }

    // 2) Poll until within eps or timeout.
    let mut last: Rect;
    let mut waited = 0u64;
    loop {
        let p = ax_get_point(win.as_ptr(), attr_pos)?;
        let s = ax_get_size(win.as_ptr(), attr_size)?;
        last = Rect {
            x: p.x,
            y: p.y,
            w: s.width,
            h: s.height,
        };
        let d = diffs(&last, target);
        if within_eps(d, eps) {
            let settle = now_ms(start);
            debug!("settle_time_ms={}", settle);
            return Ok((last, settle));
        }

        if waited >= SETTLE_TOTAL_MS {
            let settle = now_ms(start);
            debug!("settle_time_ms={}", settle);
            return Ok((last, settle));
        }

        sleep_ms(SETTLE_SLEEP_MS);
        waited = waited.saturating_add(SETTLE_SLEEP_MS);
    }
}

/// Compute the visible frame for the screen containing the given window and
/// place the window into the specified grid cell (top-left is (0,0)).
pub(crate) fn place_grid(id: WindowId, cols: u32, rows: u32, col: u32, row: u32) -> Result<()> {
    ax_check()?;
    let mtm = MainThreadMarker::new().ok_or(Error::MainThread)?;
    let (win, pid_for_id) = ax_window_for_id(id)?;
    let attr_pos = cfstr("AXPosition");
    let attr_size = cfstr("AXSize");

    (|| -> Result<()> {
        // Stage 1: normalize state (may bail for fullscreen).
        normalize_before_move(&win, pid_for_id, Some(id))?;
        let role = crate::ax::ax_get_string(win.as_ptr(), cfstr("AXRole")).unwrap_or_default();
        let subrole =
            crate::ax::ax_get_string(win.as_ptr(), cfstr("AXSubrole")).unwrap_or_default();
        let title = crate::ax::ax_get_string(win.as_ptr(), cfstr("AXTitle")).unwrap_or_default();
        let cur_p = ax_get_point(win.as_ptr(), attr_pos)?;
        let cur_s = ax_get_size(win.as_ptr(), attr_size)?;
        let (vf_x, vf_y, vf_w, vf_h) = visible_frame_containing_point(mtm, cur_p);
        let col = min(col, cols.saturating_sub(1));
        let row = min(row, rows.saturating_sub(1));
        let (x, y, w, h) = geom::grid_cell_rect(
            vf_x,
            vf_y,
            vf_w.max(1.0),
            vf_h.max(1.0),
            cols,
            rows,
            col,
            row,
        );
        let target = rect_from(x, y, w, h);
        let vf_rect = rect_from(vf_x, vf_y, vf_w, vf_h);
        debug!(
            "WinOps: place_grid: id={} pid={} role='{}' subrole='{}' title='{}' cols={} rows={} col={} row={} | cur=({:.1},{:.1},{:.1},{:.1}) vf=({:.1},{:.1},{:.1},{:.1}) target=({:.1},{:.1},{:.1},{:.1})",
            id,
            pid_for_id,
            role,
            subrole,
            title,
            cols,
            rows,
            col,
            row,
            cur_p.x,
            cur_p.y,
            cur_s.width,
            cur_s.height,
            vf_x,
            vf_y,
            vf_w,
            vf_h,
            x,
            y,
            w,
            h
        );
        // Stage 2: apply in pos->size order with settle/polling
        let (got, _settle_ms) = apply_and_wait(
            "place_grid",
            &win,
            attr_pos,
            attr_size,
            &target,
            true,
            VERIFY_EPS,
        )?;
        let d = diffs(&got, &target);
        debug!("clamp={}", clamp_flags(&got, &vf_rect, VERIFY_EPS));
        log_summary("pos->size", 1, VERIFY_EPS, d);
        if within_eps(d, VERIFY_EPS) {
            debug!("verified=true");
            debug!(
                "WinOps: place_grid verified | id={} target=({:.1},{:.1},{:.1},{:.1}) got=({:.1},{:.1},{:.1},{:.1}) diff=(dx={:.2},dy={:.2},dw={:.2},dh={:.2})",
                id,
                target.x,
                target.y,
                target.w,
                target.h,
                got.x,
                got.y,
                got.w,
                got.h,
                d.0,
                d.1,
                d.2,
                d.3
            );
            Ok(())
        } else {
            debug!("verified=false");
            Err(Error::PlacementVerificationFailed {
                op: "place_grid",
                expected: target,
                got,
                epsilon: VERIFY_EPS,
                dx: d.0,
                dy: d.1,
                dw: d.2,
                dh: d.3,
            })
        }
    })()
}

/// Place the currently focused window of `pid` into the specified grid cell on its current screen.
/// This resolves the window via Accessibility focus and performs the move immediately.
pub fn place_grid_focused(pid: i32, cols: u32, rows: u32, col: u32, row: u32) -> Result<()> {
    ax_check()?;
    let mtm = MainThreadMarker::new().ok_or(Error::MainThread)?;
    let win = crate::focused_window_for_pid(pid)?;
    let attr_pos = cfstr("AXPosition");
    let attr_size = cfstr("AXSize");

    (|| -> Result<()> {
        // Stage 1: normalize state for focused window (may bail for fullscreen).
        normalize_before_move(&win, pid, None)?;
        let role = crate::ax::ax_get_string(win.as_ptr(), cfstr("AXRole")).unwrap_or_default();
        let subrole =
            crate::ax::ax_get_string(win.as_ptr(), cfstr("AXSubrole")).unwrap_or_default();
        let title = crate::ax::ax_get_string(win.as_ptr(), cfstr("AXTitle")).unwrap_or_default();
        let cur_p = ax_get_point(win.as_ptr(), attr_pos)?;
        let cur_s = ax_get_size(win.as_ptr(), attr_size)?;
        let (vf_x, vf_y, vf_w, vf_h) = visible_frame_containing_point(mtm, cur_p);
        let col = min(col, cols.saturating_sub(1));
        let row = min(row, rows.saturating_sub(1));
        let (x, y, w, h) = geom::grid_cell_rect(
            vf_x,
            vf_y,
            vf_w.max(1.0),
            vf_h.max(1.0),
            cols,
            rows,
            col,
            row,
        );
        let target = rect_from(x, y, w, h);
        let vf_rect = rect_from(vf_x, vf_y, vf_w, vf_h);
        debug!(
            "WinOps: place_grid_focused: pid={} role='{}' subrole='{}' title='{}' cols={} rows={} col={} row={} | cur=({:.1},{:.1},{:.1},{:.1}) vf=({:.1},{:.1},{:.1},{:.1}) target=({:.1},{:.1},{:.1},{:.1})",
            pid,
            role,
            subrole,
            title,
            cols,
            rows,
            col,
            row,
            cur_p.x,
            cur_p.y,
            cur_s.width,
            cur_s.height,
            vf_x,
            vf_y,
            vf_w,
            vf_h,
            x,
            y,
            w,
            h
        );
        // Stage 2: apply in pos->size order with settle/polling
        let (got, _settle_ms) = apply_and_wait(
            "place_grid_focused",
            &win,
            attr_pos,
            attr_size,
            &target,
            true,
            VERIFY_EPS,
        )?;
        let d = diffs(&got, &target);
        debug!("clamp={}", clamp_flags(&got, &vf_rect, VERIFY_EPS));
        log_summary("pos->size", 1, VERIFY_EPS, d);
        if within_eps(d, VERIFY_EPS) {
            debug!("verified=true");
            debug!(
                "WinOps: place_grid_focused verified | pid={} target=({:.1},{:.1},{:.1},{:.1}) got=({:.1},{:.1},{:.1},{:.1}) diff=(dx={:.2},dy={:.2},dw={:.2},dh={:.2})",
                pid,
                target.x,
                target.y,
                target.w,
                target.h,
                got.x,
                got.y,
                got.w,
                got.h,
                d.0,
                d.1,
                d.2,
                d.3
            );
            Ok(())
        } else {
            debug!("verified=false");
            Err(Error::PlacementVerificationFailed {
                op: "place_grid_focused",
                expected: target,
                got,
                epsilon: VERIFY_EPS,
                dx: d.0,
                dy: d.1,
                dw: d.2,
                dh: d.3,
            })
        }
    })()
}

/// Move a window (by `id`) within a grid in the given direction.
pub(crate) fn place_move_grid(
    id: WindowId,
    cols: u32,
    rows: u32,
    dir: crate::MoveDir,
) -> Result<()> {
    ax_check()?;
    let mtm = MainThreadMarker::new().ok_or(Error::MainThread)?;
    let (win, pid_for_id) = ax_window_for_id(id)?;
    let attr_pos = cfstr("AXPosition");
    let attr_size = cfstr("AXSize");

    (|| -> Result<()> {
        // Stage 1: normalize state (may bail for fullscreen).
        normalize_before_move(&win, pid_for_id, Some(id))?;
        let role = crate::ax::ax_get_string(win.as_ptr(), cfstr("AXRole")).unwrap_or_default();
        let subrole =
            crate::ax::ax_get_string(win.as_ptr(), cfstr("AXSubrole")).unwrap_or_default();
        let title = crate::ax::ax_get_string(win.as_ptr(), cfstr("AXTitle")).unwrap_or_default();
        let cur_p = ax_get_point(win.as_ptr(), attr_pos)?;
        let cur_s = ax_get_size(win.as_ptr(), attr_size)?;
        let (vf_x, vf_y, vf_w, vf_h) = visible_frame_containing_point(mtm, cur_p);

        let eps = VERIFY_EPS;
        let cur_cell = geom::grid_find_cell(vf_x, vf_y, vf_w, vf_h, cols, rows, cur_p, cur_s, eps);

        let (next_col, next_row) = match cur_cell {
            None => (0, 0),
            Some((c, r)) => {
                let (mut nc, mut nr) = (c, r);
                match dir {
                    crate::MoveDir::Left => nc = nc.saturating_sub(1),
                    crate::MoveDir::Right => {
                        if nc + 1 < cols {
                            nc += 1;
                        }
                    }
                    crate::MoveDir::Up => nr = nr.saturating_sub(1),
                    crate::MoveDir::Down => {
                        if nr + 1 < rows {
                            nr += 1;
                        }
                    }
                }
                (nc, nr)
            }
        };

        let (x, y, w, h) =
            geom::grid_cell_rect(vf_x, vf_y, vf_w, vf_h, cols, rows, next_col, next_row);
        let target = rect_from(x, y, w, h);
        let vf_rect = rect_from(vf_x, vf_y, vf_w, vf_h);
        debug!(
            "WinOps: place_move_grid: id={} pid={} role='{}' subrole='{}' title='{}' cols={} rows={} dir={:?} | cur=({:.1},{:.1},{:.1},{:.1}) vf=({:.1},{:.1},{:.1},{:.1}) cur_cell={:?} next_cell=({}, {}) target=({:.1},{:.1},{:.1},{:.1})",
            id,
            pid_for_id,
            role,
            subrole,
            title,
            cols,
            rows,
            dir,
            cur_p.x,
            cur_p.y,
            cur_s.width,
            cur_s.height,
            vf_x,
            vf_y,
            vf_w,
            vf_h,
            cur_cell,
            next_col,
            next_row,
            x,
            y,
            w,
            h
        );

        // Stage 2: apply in pos->size order with settle/polling
        let (got, _settle_ms) = apply_and_wait(
            "place_move_grid",
            &win,
            attr_pos,
            attr_size,
            &target,
            true,
            VERIFY_EPS,
        )?;
        let d = diffs(&got, &target);
        debug!("clamp={}", clamp_flags(&got, &vf_rect, VERIFY_EPS));
        log_summary("pos->size", 1, VERIFY_EPS, d);
        if within_eps(d, VERIFY_EPS) {
            debug!("verified=true");
            debug!(
                "WinOps: place_move_grid verified | id={} target=({:.1},{:.1},{:.1},{:.1}) got=({:.1},{:.1},{:.1},{:.1}) diff=(dx={:.2},dy={:.2},dw={:.2},dh={:.2})",
                id,
                target.x,
                target.y,
                target.w,
                target.h,
                got.x,
                got.y,
                got.w,
                got.h,
                d.0,
                d.1,
                d.2,
                d.3
            );
            Ok(())
        } else {
            debug!("verified=false");
            Err(Error::PlacementVerificationFailed {
                op: "place_move_grid",
                expected: target,
                got,
                epsilon: VERIFY_EPS,
                dx: d.0,
                dy: d.1,
                dw: d.2,
                dh: d.3,
            })
        }
    })()
}
