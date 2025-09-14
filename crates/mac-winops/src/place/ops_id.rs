use std::cmp::min;

use objc2_foundation::MainThreadMarker;
use tracing::debug;

use super::{
    apply::apply_and_wait,
    common::{
        VERIFY_EPS, clamp_flags, diffs, log_failure_context, log_summary, rect_from, within_eps,
    },
    fallback::{fallback_shrink_move_grow, needs_safe_park, preflight_safe_park},
    normalize::{normalize_before_move, skip_reason_for_role_subrole},
};
use crate::{
    Error, Result, WindowId,
    ax::{ax_check, ax_get_point, ax_get_size, ax_window_for_id, cfstr},
    geom::{self, Rect},
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
        // Stage 5: compute a local rect relative to the chosen visible frame and
        // convert to global coordinates explicitly.
        let target_local = Rect {
            x: x - vf_x,
            y: y - vf_y,
            w,
            h,
        };
        let g = crate::screen_util::globalize_rect(target_local, vf_x, vf_y);
        debug!(
            "coordspace: local=({:.1},{:.1},{:.1},{:.1}) + origin=({:.1},{:.1}) -> global=({:.1},{:.1},{:.1},{:.1})",
            target_local.x,
            target_local.y,
            target_local.w,
            target_local.h,
            vf_x,
            vf_y,
            g.x,
            g.y,
            g.w,
            g.h
        );
        let target = rect_from(g.x, g.y, g.w, g.h);
        // Stage 3.1: if we would trip coordinate-space issues near global (0,0)
        if needs_safe_park(&target, vf_x, vf_y) {
            preflight_safe_park("place_grid", &win, attr_pos, attr_size, vf_x, vf_y, &target)?;
        }
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
        let (vf2_x, vf2_y, vf2_w, vf2_h) = visible_frame_containing_point(
            mtm,
            geom::CGPoint {
                x: got1.cx(),
                y: got1.cy(),
            },
        );
        let vf2_rect = rect_from(vf2_x, vf2_y, vf2_w, vf2_h);
        debug!(
            "vf_used:center=({:.1},{:.1}) -> vf=({:.1},{:.1},{:.1},{:.1})",
            got1.cx(),
            got1.cy(),
            vf2_x,
            vf2_y,
            vf2_w,
            vf2_h
        );
        debug!("clamp={}", clamp_flags(&got1, &vf2_rect, VERIFY_EPS));
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
                "WinOps: place_grid verified | id={} target=({:.1},{:.1},{:.1},{:.1}) got=({:.1},{:.1},{:.1},{:.1}) diff=(dx={:.2},dy={:.2},dw={:.2},dh={:.2})",
                id,
                target.x,
                target.y,
                target.w,
                target.h,
                got1.x,
                got1.y,
                got1.w,
                got1.h,
                d1.0,
                d1.1,
                d1.2,
                d1.3
            );
            Ok(())
        } else {
            if pos_first_only {
                debug!("verified=false");
                log_failure_context(&win, &role, &subrole);
                let clamped = clamp_flags(&got1, &vf2_rect, VERIFY_EPS);
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
                let (vf3_x, vf3_y, vf3_w, vf3_h) = visible_frame_containing_point(
                    mtm,
                    geom::CGPoint {
                        x: got_ax.cx(),
                        y: got_ax.cy(),
                    },
                );
                let vf3_rect = rect_from(vf3_x, vf3_y, vf3_w, vf3_h);
                debug!(
                    "vf_used:center=({:.1},{:.1}) -> vf=({:.1},{:.1},{:.1},{:.1})",
                    got_ax.cx(),
                    got_ax.cy(),
                    vf3_x,
                    vf3_y,
                    vf3_w,
                    vf3_h
                );
                debug!("clamp={}", clamp_flags(&got_ax, &vf3_rect, VERIFY_EPS));
                let label = match axis {
                    super::common::Axis::X => "axis-pos:x",
                    super::common::Axis::Y => "axis-pos:y",
                };
                log_summary(label, attempt_idx, VERIFY_EPS, dax);
                if within_eps(dax, VERIFY_EPS) {
                    debug!("verified=true");
                    debug!("order_used=axis-pos, attempts=2");
                    debug!(
                        "WinOps: place_grid verified | id={} target=({:.1},{:.1},{:.1},{:.1}) got=({:.1},{:.1},{:.1},{:.1}) diff=(dx={:.2},dy={:.2},dw={:.2},dh={:.2})",
                        id,
                        target.x,
                        target.y,
                        target.w,
                        target.h,
                        got_ax.x,
                        got_ax.y,
                        got_ax.w,
                        got_ax.h,
                        dax.0,
                        dax.1,
                        dax.2,
                        dax.3
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
            let (vf4_x, vf4_y, vf4_w, vf4_h) = visible_frame_containing_point(
                mtm,
                geom::CGPoint {
                    x: got2.cx(),
                    y: got2.cy(),
                },
            );
            let vf4_rect = rect_from(vf4_x, vf4_y, vf4_w, vf4_h);
            debug!(
                "vf_used:center=({:.1},{:.1}) -> vf=({:.1},{:.1},{:.1},{:.1})",
                got2.cx(),
                got2.cy(),
                vf4_x,
                vf4_y,
                vf4_w,
                vf4_h
            );
            debug!("clamp={}", clamp_flags(&got2, &vf4_rect, VERIFY_EPS));
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
                        "WinOps: place_grid verified | id={} target=({:.1},{:.1},{:.1},{:.1}) got=({:.1},{:.1},{:.1},{:.1}) diff=(dx={:.2},dy={:.2},dw={:.2},dh={:.2})",
                        id,
                        target.x,
                        target.y,
                        target.w,
                        target.h,
                        got3.x,
                        got3.y,
                        got3.w,
                        got3.h,
                        d3.0,
                        d3.1,
                        d3.2,
                        d3.3
                    );
                    Ok(())
                } else {
                    debug!("verified=false");
                    log_failure_context(&win, &role, &subrole);
                    let (vfx, vfy, vfw, vfh) = visible_frame_containing_point(
                        mtm,
                        geom::CGPoint {
                            x: got3.cx(),
                            y: got3.cy(),
                        },
                    );
                    debug!(
                        "vf_used:center=({:.1},{:.1}) -> vf=({:.1},{:.1},{:.1},{:.1})",
                        got3.cx(),
                        got3.cy(),
                        vfx,
                        vfy,
                        vfw,
                        vfh
                    );
                    let vf_final = rect_from(vfx, vfy, vfw, vfh);
                    let clamped = clamp_flags(&got3, &vf_final, VERIFY_EPS);
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
                    "WinOps: place_grid verified | id={} target=({:.1},{:.1},{:.1},{:.1}) got=({:.1},{:.1},{:.1},{:.1}) diff=(dx={:.2},dy={:.2},dw={:.2},dh={:.2})",
                    id,
                    target.x,
                    target.y,
                    target.w,
                    target.h,
                    got2.x,
                    got2.y,
                    got2.w,
                    got2.h,
                    d2.0,
                    d2.1,
                    d2.2,
                    d2.3
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
                        "WinOps: place_grid verified | id={} target=({:.1},{:.1},{:.1},{:.1}) got=({:.1},{:.1},{:.1},{:.1}) diff=(dx={:.2},dy={:.2},dw={:.2},dh={:.2})",
                        id,
                        target.x,
                        target.y,
                        target.w,
                        target.h,
                        got3.x,
                        got3.y,
                        got3.w,
                        got3.h,
                        d3.0,
                        d3.1,
                        d3.2,
                        d3.3
                    );
                    Ok(())
                } else {
                    debug!("verified=false");
                    log_failure_context(&win, &role, &subrole);
                    let (vfx, vfy, vfw, vfh) = visible_frame_containing_point(
                        mtm,
                        geom::CGPoint {
                            x: got3.cx(),
                            y: got3.cy(),
                        },
                    );
                    debug!(
                        "vf_used:center=({:.1},{:.1}) -> vf=({:.1},{:.1},{:.1},{:.1})",
                        got3.cx(),
                        got3.cy(),
                        vfx,
                        vfy,
                        vfw,
                        vfh
                    );
                    let vf_final = rect_from(vfx, vfy, vfw, vfh);
                    let clamped = clamp_flags(&got3, &vf_final, VERIFY_EPS);
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
