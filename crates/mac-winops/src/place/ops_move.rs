use objc2_foundation::MainThreadMarker;
use tracing::{debug, warn};

use super::{
    apply::AxAttrRefs,
    common::{PlaceAttemptOptions, PlacementContext, trace_safe_park},
    engine::{PlacementEngine, PlacementEngineConfig, PlacementGrid, PlacementOutcome},
    normalize::{normalize_before_move, skip_reason_for_role_subrole},
};
use crate::{
    Error, Result, WindowId,
    ax::{ax_check, ax_get_point, ax_get_size, cfstr},
    error::PlacementErrorDetails,
    geom::Rect,
    screen_util::visible_frame_containing_point,
};

/// Move a window (by `id`) within a grid in the given direction using default placement options.
pub fn place_move_grid(id: WindowId, cols: u32, rows: u32, dir: crate::MoveDir) -> Result<()> {
    place_move_grid_inner(id, cols, rows, dir, PlaceAttemptOptions::default())
}

/// Move a window (by `id`) within a grid in the given direction with explicit options.
pub(crate) fn place_move_grid_opts(
    id: WindowId,
    cols: u32,
    rows: u32,
    dir: crate::MoveDir,
    opts: PlaceAttemptOptions,
) -> Result<()> {
    place_move_grid_inner(id, cols, rows, dir, opts)
}

fn place_move_grid_inner(
    id: WindowId,
    cols: u32,
    rows: u32,
    dir: crate::MoveDir,
    opts: PlaceAttemptOptions,
) -> Result<()> {
    ax_check()?;
    let mtm = MainThreadMarker::new().ok_or(Error::MainThread)?;
    let (win, pid_for_id) = super::ax_window_for_id_with_retry(id)?;
    let attr_pos = cfstr("AXPosition");
    let attr_size = cfstr("AXSize");

    (|| -> Result<()> {
        // Stage 1: normalize state (may bail for fullscreen).
        normalize_before_move(&win, pid_for_id, Some(id))?;
        let role = crate::ax::ax_get_string(win.as_ptr(), cfstr("AXRole")).unwrap_or_default();
        let subrole =
            crate::ax::ax_get_string(win.as_ptr(), cfstr("AXSubrole")).unwrap_or_default();
        let title = crate::ax::ax_get_string(win.as_ptr(), cfstr("AXTitle")).unwrap_or_default();
        if let Some(reason) = skip_reason_for_role_subrole(&role, &subrole) {
            debug!(
                "skipped: role/subrole reason={} role='{}' subrole='{}' title='{}'",
                reason, role, subrole, title
            );
            return Ok(());
        }
        let cur_p = ax_get_point(win.as_ptr(), attr_pos)?;
        let cur_s = ax_get_size(win.as_ptr(), attr_size)?;
        let vf = visible_frame_containing_point(mtm, cur_p);

        let eps = opts.tuning().epsilon();
        let cur_cell = vf.grid_find_cell(cols, rows, cur_p, cur_s, eps);

        let (next_col, next_row) = match cur_cell {
            None => {
                // Fallback: infer current cell by position only, ignoring size mismatches
                let (mut nc, mut nr) =
                    super::grid_guess_cell_by_pos(vf.x, vf.y, vf.w, vf.h, cols, rows, cur_p);
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

        let Rect { x, y, w, h } = vf.grid_cell(cols, rows, next_col, next_row);
        let target_local = Rect {
            x: x - vf.x,
            y: y - vf.y,
            w,
            h,
        };
        let g = crate::screen_util::globalize_rect(target_local, vf.x, vf.y);
        debug!(
            "coordspace: local={} + origin=({:.1},{:.1}) -> global={}",
            target_local, vf.x, vf.y, g
        );
        let target = Rect::new(g.x, g.y, g.w, g.h);
        let ctx = PlacementContext::new(win.clone(), target, vf, opts);
        let vf = *ctx.visible_frame();
        let target = *ctx.target();
        let opts = ctx.attempt_options();
        let tuning = opts.tuning();
        let adapter = ctx.adapter();
        let engine = PlacementEngine::new(
            &ctx,
            PlacementEngineConfig {
                label: "place_move_grid",
                attr_pos,
                attr_size,
                grid: PlacementGrid {
                    cols,
                    rows,
                    col: next_col,
                    row: next_row,
                },
                role: &role,
                subrole: &subrole,
            },
        );
        if opts.hooks().should_safe_park(&ctx) {
            trace_safe_park("place_move_grid");
            adapter.preflight_safe_park(
                "place_move_grid",
                ctx.win(),
                AxAttrRefs {
                    pos: attr_pos,
                    size: attr_size,
                },
                &vf,
                &target,
                tuning.epsilon(),
                tuning.settle_timing(),
            )?;
        }
        let cur = Rect::from((cur_p, cur_s));
        debug!(
            "WinOps: place_move_grid: id={} pid={} role='{}' subrole='{}' title='{}' cols={} rows={} dir={:?} | cur={} vf={} cur_cell={:?} next_cell=({}, {}) target={}",
            id,
            pid_for_id,
            role,
            subrole,
            title,
            cols,
            rows,
            dir,
            cur,
            vf,
            cur_cell,
            next_col,
            next_row,
            target
        );

        match engine.execute(mtm)? {
            PlacementOutcome::Verified(success) => {
                if let Some(anchored) = success.anchored_target {
                    debug!(
                        "WinOps: place_move_grid verified (anchored legal) | id={} anchored={} got={}",
                        id, anchored, success.final_rect
                    );
                } else {
                    debug!(
                        "WinOps: place_move_grid verified | id={} target={} got={}",
                        id, target, success.final_rect
                    );
                }
                Ok(())
            }
            PlacementOutcome::PosFirstOnlyFailure(failure) => {
                warn!(
                    "PlaceMoveGrid failed (pos-first-only): id={} expected={} got={} clamp={}",
                    id, target, failure.got, failure.clamped
                );
                let _ = crate::raise::raise_window(pid_for_id, id);
                Err(Error::PlacementVerificationFailed {
                    op: "place_move_grid",
                    details: Box::new(PlacementErrorDetails {
                        expected: target,
                        got: failure.got,
                        epsilon: tuning.epsilon(),
                        clamped: failure.clamped,
                        visible_frame: failure.visible_frame,
                        timeline: failure.timeline,
                    }),
                })
            }
            PlacementOutcome::VerificationFailure(failure) => {
                warn!(
                    "PlaceMoveGrid failed: id={} expected={} got={} clamp={}",
                    id, target, failure.got, failure.clamped
                );
                let _ = crate::raise::raise_window(pid_for_id, id);
                Err(Error::PlacementVerificationFailed {
                    op: "place_move_grid",
                    details: Box::new(PlacementErrorDetails {
                        expected: target,
                        got: failure.got,
                        epsilon: tuning.epsilon(),
                        clamped: failure.clamped,
                        visible_frame: failure.visible_frame,
                        timeline: failure.timeline,
                    }),
                })
            }
        }
    })()
}
