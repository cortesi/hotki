use std::cmp::min;

use objc2_foundation::MainThreadMarker;
use tracing::debug;

use crate::{
    Error, Result, WindowId,
    ax::{ax_check, ax_get_point, ax_get_size, ax_set_point, ax_set_size, ax_window_for_id, cfstr},
    geom::{self, CGPoint, CGSize, Rect},
    screen_util::visible_frame_containing_point,
};

/// Epsilon tolerance (in points) used to verify post‑placement position and size.
const VERIFY_EPS: f64 = 2.0;

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

/// Compute the visible frame for the screen containing the given window and
/// place the window into the specified grid cell (top-left is (0,0)).
pub(crate) fn place_grid(id: WindowId, cols: u32, rows: u32, col: u32, row: u32) -> Result<()> {
    ax_check()?;
    let mtm = MainThreadMarker::new().ok_or(Error::MainThread)?;
    let (win, _pid_for_id) = ax_window_for_id(id)?;
    let attr_pos = cfstr("AXPosition");
    let attr_size = cfstr("AXSize");

    (|| -> Result<()> {
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
        debug!(
            "WinOps: place_grid: id={} cols={} rows={} col={} row={} | cur=({:.1},{:.1},{:.1},{:.1}) vf=({:.1},{:.1},{:.1},{:.1}) target=({:.1},{:.1},{:.1},{:.1})",
            id,
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
        debug!("WinOps: place_grid set pos -> ({:.1},{:.1})", x, y);
        ax_set_point(win.as_ptr(), attr_pos, CGPoint { x, y })?;
        debug!("WinOps: place_grid set size -> ({:.1},{:.1})", w, h);
        ax_set_size(
            win.as_ptr(),
            attr_size,
            CGSize {
                width: w,
                height: h,
            },
        )?;

        // Post‑placement verification
        let got_p = ax_get_point(win.as_ptr(), attr_pos)?;
        let got_s = ax_get_size(win.as_ptr(), attr_size)?;
        let got = rect_from(got_p.x, got_p.y, got_s.width, got_s.height);
        let d = diffs(&got, &target);
        if within_eps(d, VERIFY_EPS) {
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
        debug!(
            "WinOps: place_grid_focused: pid={} cols={} rows={} col={} row={} | cur=({:.1},{:.1},{:.1},{:.1}) vf=({:.1},{:.1},{:.1},{:.1}) target=({:.1},{:.1},{:.1},{:.1})",
            pid,
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
        debug!("WinOps: place_grid_focused set pos -> ({:.1},{:.1})", x, y);
        ax_set_point(win.as_ptr(), attr_pos, CGPoint { x, y })?;
        debug!("WinOps: place_grid_focused set size -> ({:.1},{:.1})", w, h);
        ax_set_size(
            win.as_ptr(),
            attr_size,
            CGSize {
                width: w,
                height: h,
            },
        )?;

        // Post‑placement verification
        let got_p = ax_get_point(win.as_ptr(), attr_pos)?;
        let got_s = ax_get_size(win.as_ptr(), attr_size)?;
        let got = rect_from(got_p.x, got_p.y, got_s.width, got_s.height);
        let d = diffs(&got, &target);
        if within_eps(d, VERIFY_EPS) {
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
    let (win, _pid_for_id) = ax_window_for_id(id)?;
    let attr_pos = cfstr("AXPosition");
    let attr_size = cfstr("AXSize");

    (|| -> Result<()> {
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
        debug!(
            "WinOps: place_move_grid: id={} cols={} rows={} dir={:?} | cur=({:.1},{:.1},{:.1},{:.1}) vf=({:.1},{:.1},{:.1},{:.1}) cur_cell={:?} next_cell=({}, {}) target=({:.1},{:.1},{:.1},{:.1})",
            id,
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

        debug!("WinOps: place_move_grid set pos -> ({:.1},{:.1})", x, y);
        ax_set_point(win.as_ptr(), attr_pos, CGPoint { x, y })?;
        debug!("WinOps: place_move_grid set size -> ({:.1},{:.1})", w, h);
        ax_set_size(
            win.as_ptr(),
            attr_size,
            CGSize {
                width: w,
                height: h,
            },
        )?;

        // Post‑placement verification
        let got_p = ax_get_point(win.as_ptr(), attr_pos)?;
        let got_s = ax_get_size(win.as_ptr(), attr_size)?;
        let got = rect_from(got_p.x, got_p.y, got_s.width, got_s.height);
        let d = diffs(&got, &target);
        if within_eps(d, VERIFY_EPS) {
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
