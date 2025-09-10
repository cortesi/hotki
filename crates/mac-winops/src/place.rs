use crate::{
    Error, Result, WindowId,
    ax::{ax_check, ax_get_point, ax_get_size, ax_set_point, ax_set_size, cfstr},
    geom::{self, CGPoint, CGSize},
    screen_util::visible_frame_containing_point,
};
use core_foundation::base::{CFRelease, CFTypeRef};
use objc2_foundation::MainThreadMarker;

use crate::ax::ax_window_for_id;

/// Compute the visible frame for the screen containing the given window and
/// place the window into the specified grid cell (top-left is (0,0)).
pub(crate) fn place_grid(id: WindowId, cols: u32, rows: u32, col: u32, row: u32) -> Result<()> {
    ax_check()?;
    let mtm = MainThreadMarker::new().ok_or(Error::MainThread)?;
    let (win, _pid_for_id) = ax_window_for_id(id)?;
    let attr_pos = cfstr("AXPosition");
    let attr_size = cfstr("AXSize");

    let result = (|| -> Result<()> {
        let cur_p = ax_get_point(win, attr_pos)?;
        let (vf_x, vf_y, vf_w, vf_h) = visible_frame_containing_point(mtm, cur_p);
        let col = col.min(cols.saturating_sub(1));
        let row = row.min(rows.saturating_sub(1));
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
        ax_set_point(win, attr_pos, CGPoint { x, y })?;
        ax_set_size(
            win,
            attr_size,
            CGSize {
                width: w,
                height: h,
            },
        )?;
        Ok(())
    })();
    unsafe { CFRelease(win as CFTypeRef) };
    result
}

/// Place the currently focused window of `pid` into the specified grid cell on its current screen.
/// This resolves the window via Accessibility focus and performs the move immediately.
pub fn place_grid_focused(pid: i32, cols: u32, rows: u32, col: u32, row: u32) -> Result<()> {
    ax_check()?;
    let mtm = MainThreadMarker::new().ok_or(Error::MainThread)?;
    let win = crate::focused_window_for_pid(pid)?;
    let attr_pos = cfstr("AXPosition");
    let attr_size = cfstr("AXSize");
    let result = (|| -> Result<()> {
        let cur_p = ax_get_point(win, attr_pos)?;
        let cur_s = ax_get_size(win, attr_size)?;
        let (vf_x, vf_y, vf_w, vf_h) = visible_frame_containing_point(mtm, cur_p);
        let col = col.min(cols.saturating_sub(1));
        let row = row.min(rows.saturating_sub(1));
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
        tracing::debug!(
            "WinOps: place_grid_focused current x={:.1} y={:.1} w={:.1} h={:.1} | target x={:.1} y={:.1} w={:.1} h={:.1}",
            cur_p.x,
            cur_p.y,
            cur_s.width,
            cur_s.height,
            x,
            y,
            w,
            h
        );
        ax_set_point(win, attr_pos, CGPoint { x, y })?;
        ax_set_size(
            win,
            attr_size,
            CGSize {
                width: w,
                height: h,
            },
        )?;
        Ok(())
    })();
    unsafe { CFRelease(win as CFTypeRef) };
    result
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

    let result = (|| -> Result<()> {
        let cur_p = ax_get_point(win, attr_pos)?;
        let cur_s = ax_get_size(win, attr_size)?;
        let (vf_x, vf_y, vf_w, vf_h) = visible_frame_containing_point(mtm, cur_p);

        let eps = 2.0;
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
        ax_set_point(win, attr_pos, CGPoint { x, y })?;
        ax_set_size(
            win,
            attr_size,
            CGSize {
                width: w,
                height: h,
            },
        )?;
        Ok(())
    })();
    unsafe { CFRelease(win as CFTypeRef) };
    result
}
