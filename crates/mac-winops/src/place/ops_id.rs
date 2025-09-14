use std::cmp::min;

use objc2_foundation::MainThreadMarker;
use tracing::debug;

use super::{
    apply::apply_and_wait,
    common::{VERIFY_EPS, clamp_flags, log_failure_context, log_summary},
    fallback::{fallback_shrink_move_grow, needs_safe_park, preflight_safe_park},
    normalize::{normalize_before_move, skip_reason_for_role_subrole},
};
use crate::{
    Error, Result, WindowId,
    ax::{ax_check, ax_get_point, ax_get_size, ax_window_for_id, cfstr},
    geom::{self, Rect, diffs, rect_from, within_eps},
    screen_util::visible_frame_containing_point,
};

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
        let col = min(col, cols.saturating_sub(1));
        let row = min(row, rows.saturating_sub(1));
        let (x, y, w, h) = geom::grid_cell_rect(
            vf.x,
            vf.y,
            vf.w.max(1.0),
            vf.h.max(1.0),
            cols,
            rows,
            col,
            row,
        );
        // Stage 5: compute a local rect relative to the chosen visible frame and
        // convert to global coordinates explicitly.
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
        let target = rect_from(g.x, g.y, g.w, g.h);
        // Stage 3.1: if we would trip coordinate-space issues near global (0,0)
        if needs_safe_park(&target, vf.x, vf.y) {
            preflight_safe_park("place_grid", &win, attr_pos, attr_size, vf.x, vf.y, &target)?;
        }
        let cur = Rect::from((cur_p, cur_s));
        debug!(
            "WinOps: place_grid: id={} pid={} role='{}' subrole='{}' title='{}' cols={} rows={} col={} row={} | cur={} vf={} target={}",
            id, pid_for_id, role, subrole, title, cols, rows, col, row, cur, vf, target
        );
        // Stage 2: choose initial order from cached settable bits; if that
        // does not converge within eps, retry with the opposite order (Stage 3).
        let force_second = false;
        let pos_first_only = false;
        let (can_pos, can_size) = crate::ax::ax_settable_pos_size(win.as_ptr());
        let initial_pos_first = super::common::choose_initial_order(can_pos, can_size);
        debug!(
            "order_hint: settable_pos={:?} settable_size={:?} -> initial={}",
            can_pos,
            can_size,
            if initial_pos_first {
                "pos->size"
            } else {
                "size->pos"
            }
        );
        let (got1, _settle_ms1) = apply_and_wait(
            "place_grid",
            &win,
            attr_pos,
            attr_size,
            &target,
            initial_pos_first,
            VERIFY_EPS,
        )?;
        let d1 = diffs(&got1, &target);
        // Stage 7.2: validate against final screen selected by window center
        let vf2 = visible_frame_containing_point(
            mtm,
            geom::CGPoint {
                x: got1.cx(),
                y: got1.cy(),
            },
        );
        debug!("vf_used:center={} -> vf={}", got1.center(), vf2);
        debug!("clamp={}", clamp_flags(&got1, &vf2, VERIFY_EPS));
        log_summary(
            if initial_pos_first {
                "pos->size"
            } else {
                "size->pos"
            },
            1,
            VERIFY_EPS,
            d1,
        );
        if within_eps(d1, VERIFY_EPS) && !force_second {
            debug!("verified=true");
            debug!(
                "WinOps: place_grid verified | id={} target={} got={} diff=(dx={:.2},dy={:.2},dw={:.2},dh={:.2})",
                id, target, got1, d1.0, d1.1, d1.2, d1.3
            );
            Ok(())
        } else {
            if pos_first_only {
                debug!("verified=false");
                log_failure_context(&win, &role, &subrole);
                let clamped = clamp_flags(&got1, &vf2, VERIFY_EPS);
                return Err(Error::PlacementVerificationFailed {
                    op: "place_grid",
                    expected: target,
                    got: got1,
                    epsilon: VERIFY_EPS,
                    dx: d1.0,
                    dy: d1.1,
                    dw: d1.2,
                    dh: d1.3,
                    clamped,
                });
            }
            // Stage 7.1: If only one axis is off, try a single-axis nudge first.
            let mut attempt_idx = 2u32;
            if let Some(axis) = super::common::one_axis_off(d1, VERIFY_EPS) {
                let (got_ax, _settle_ms_ax) = super::apply::nudge_axis_pos_and_wait(
                    "place_grid",
                    &win,
                    attr_pos,
                    attr_size,
                    &target,
                    axis,
                    VERIFY_EPS,
                )?;
                let dax = diffs(&got_ax, &target);
                let vf3 = visible_frame_containing_point(
                    mtm,
                    geom::CGPoint {
                        x: got_ax.cx(),
                        y: got_ax.cy(),
                    },
                );
                debug!("vf_used:center={} -> vf={}", got_ax.center(), vf3);
                debug!("clamp={}", clamp_flags(&got_ax, &vf3, VERIFY_EPS));
                let label = match axis {
                    crate::geom::Axis::Horizontal => "axis-pos:x",
                    crate::geom::Axis::Vertical => "axis-pos:y",
                };
                log_summary(label, attempt_idx, VERIFY_EPS, dax);
                if within_eps(dax, VERIFY_EPS) {
                    debug!("verified=true");
                    debug!("order_used=axis-pos, attempts=2");
                    debug!(
                        "WinOps: place_grid verified | id={} target={} got={} diff=(dx={:.2},dy={:.2},dw={:.2},dh={:.2})",
                        id, target, got_ax, dax.0, dax.1, dax.2, dax.3
                    );
                    return Ok(());
                }
                attempt_idx = 3;
            }
            // Stage 3: retry size->pos
            let (got2, _settle_ms2) = apply_and_wait(
                "place_grid",
                &win,
                attr_pos,
                attr_size,
                &target,
                false,
                VERIFY_EPS,
            )?;
            let d2 = diffs(&got2, &target);
            let vf4 = visible_frame_containing_point(
                mtm,
                geom::CGPoint {
                    x: got2.cx(),
                    y: got2.cy(),
                },
            );
            debug!("vf_used:center={} -> vf={}", got2.center(), vf4);
            debug!("clamp={}", clamp_flags(&got2, &vf4, VERIFY_EPS));
            log_summary("size->pos", attempt_idx, VERIFY_EPS, d2);
            let force_smg = false;
            if force_smg {
                debug!("fallback_used=true");
                let got3 =
                    fallback_shrink_move_grow("place_grid", &win, attr_pos, attr_size, &target)?;
                let d3 = diffs(&got3, &target);
                if within_eps(d3, VERIFY_EPS) {
                    debug!("verified=true");
                    debug!("order_used=shrink->move->grow, attempts=3");
                    debug!(
                        "WinOps: place_grid verified | id={} target={} got={} diff=(dx={:.2},dy={:.2},dw={:.2},dh={:.2})",
                        id, target, got3, d3.0, d3.1, d3.2, d3.3
                    );
                    Ok(())
                } else {
                    debug!("verified=false");
                    log_failure_context(&win, &role, &subrole);
                    let vf = visible_frame_containing_point(
                        mtm,
                        geom::CGPoint {
                            x: got3.cx(),
                            y: got3.cy(),
                        },
                    );
                    debug!("vf_used:center={} -> vf={}", got3.center(), vf);
                    let clamped = clamp_flags(&got3, &vf, VERIFY_EPS);
                    Err(Error::PlacementVerificationFailed {
                        op: "place_grid",
                        expected: target,
                        got: got3,
                        epsilon: VERIFY_EPS,
                        dx: d3.0,
                        dy: d3.1,
                        dw: d3.2,
                        dh: d3.3,
                        clamped,
                    })
                }
            } else if within_eps(d2, VERIFY_EPS) {
                debug!("verified=true");
                debug!("order_used=size->pos, attempts={}", attempt_idx);
                debug!(
                    "WinOps: place_grid verified | id={} target={} got={} diff=(dx={:.2},dy={:.2},dw={:.2},dh={:.2})",
                    id, target, got2, d2.0, d2.1, d2.2, d2.3
                );
                Ok(())
            } else {
                // Stage 4: shrink→move→grow fallback
                debug!("fallback_used=true");
                let got3 =
                    fallback_shrink_move_grow("place_grid", &win, attr_pos, attr_size, &target)?;
                let d3 = diffs(&got3, &target);
                if within_eps(d3, VERIFY_EPS) {
                    debug!("verified=true");
                    debug!("order_used=shrink->move->grow, attempts=3");
                    debug!(
                        "WinOps: place_grid verified | id={} target={} got={} diff=(dx={:.2},dy={:.2},dw={:.2},dh={:.2})",
                        id, target, got3, d3.0, d3.1, d3.2, d3.3
                    );
                    Ok(())
                } else {
                    debug!("verified=false");
                    log_failure_context(&win, &role, &subrole);
                    let vf = visible_frame_containing_point(
                        mtm,
                        geom::CGPoint {
                            x: got3.cx(),
                            y: got3.cy(),
                        },
                    );
                    debug!(
                        "vf_used:center=({:.1},{:.1}) -> vf={}",
                        got3.cx(),
                        got3.cy(),
                        vf
                    );
                    let clamped = clamp_flags(&got3, &vf, VERIFY_EPS);
                    Err(Error::PlacementVerificationFailed {
                        op: "place_grid",
                        expected: target,
                        got: got3,
                        epsilon: VERIFY_EPS,
                        dx: d3.0,
                        dy: d3.1,
                        dw: d3.2,
                        dh: d3.3,
                        clamped,
                    })
                }
            }
        }
    })()
}
