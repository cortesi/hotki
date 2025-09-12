use core_foundation::base::{CFRelease, CFTypeRef};
use objc2_foundation::MainThreadMarker;

use crate::{
    Error, Result,
    ax::{ax_check, ax_get_point, ax_get_size, ax_window_for_id, cfstr},
    geom::{self},
    list_windows, request_activate_pid,
    window::frontmost_window,
};

/// Focus the next window in the given direction on the current screen within the
/// current Space. Uses CG for enumeration + AppKit for screen geometry and AX for
/// the origin window frame and final raise.
pub(crate) fn focus_dir(dir: crate::MoveDir) -> Result<()> {
    ax_check()?;
    let _mtm = MainThreadMarker::new().ok_or(Error::MainThread)?;

    let origin = match frontmost_window() {
        Some(w) => w,
        None => return Err(Error::FocusedWindow),
    };

    (|| -> Result<()> {
        let (ax_origin, _pid_for_id) = ax_window_for_id(origin.id)?;
        let o_pos = ax_get_point(ax_origin, cfstr("AXPosition"))?;
        let o_size = ax_get_size(ax_origin, cfstr("AXSize"))?;
        let o_rect = geom::Rect {
            x: o_pos.x,
            y: o_pos.y,
            w: o_size.width.max(1.0),
            h: o_size.height.max(1.0),
        };
        let o_cx = o_rect.cx();
        let o_cy = o_rect.cy();
        tracing::info!(
            "FocusDir origin(AX): pid={} id={} x={:.1} y={:.1} w={:.1} h={:.1}",
            origin.pid,
            origin.id,
            o_rect.x,
            o_rect.y,
            o_rect.w,
            o_rect.h
        );

        let eps = 16.0f64;
        let all = list_windows();
        let mut primary_best_same: Option<(i32, f64, f64, i32, crate::WindowId)> = None;
        let mut primary_best_other: Option<(i32, f64, f64, i32, crate::WindowId)> = None;
        let mut fallback_best_same: Option<(i32, f64, i32, crate::WindowId)> = None;
        let mut fallback_best_other: Option<(i32, f64, i32, crate::WindowId)> = None;

        for w in all.into_iter() {
            if w.layer != 0 {
                continue;
            }
            if w.pid == origin.pid && w.id == origin.id {
                continue;
            }
            if let Some(s) = origin.space
                && let Some(ws) = w.space
                && ws != s
            {
                continue;
            }
            let (cand_left, cand_bottom, cand_w, cand_h, id_match) = {
                match ax_window_for_id(w.id) {
                    Ok((cax, _)) => {
                        let mut id_match = false;
                        unsafe {
                            use core_foundation::base::TCFType;
                            let mut num_ref: CFTypeRef = std::ptr::null_mut();
                            let nerr = crate::ax::AXUIElementCopyAttributeValue(
                                cax,
                                cfstr("AXWindowNumber"),
                                &mut num_ref,
                            );
                            if nerr == 0 && !num_ref.is_null() {
                                let cfnum =
                                    core_foundation::number::CFNumber::wrap_under_create_rule(
                                        num_ref as _,
                                    );
                                let wid = cfnum.to_i64().unwrap_or(0) as u32;
                                if wid == w.id {
                                    id_match = true;
                                }
                            }
                        }
                        let p = match ax_get_point(cax, cfstr("AXPosition")) {
                            Ok(v) => v,
                            Err(_) => {
                                unsafe { CFRelease(cax as CFTypeRef) };
                                continue;
                            }
                        };
                        let s = match ax_get_size(cax, cfstr("AXSize")) {
                            Ok(v) => v,
                            Err(_) => {
                                unsafe { CFRelease(cax as CFTypeRef) };
                                continue;
                            }
                        };
                        unsafe { CFRelease(cax as CFTypeRef) };
                        (p.x, p.y, s.width.max(1.0), s.height.max(1.0), id_match)
                    }
                    Err(_) => continue,
                }
            };
            let c_rect = geom::Rect {
                x: cand_left,
                y: cand_bottom,
                w: cand_w,
                h: cand_h,
            };
            let cx = c_rect.cx();
            let cy = c_rect.cy();

            let same_row = geom::same_row_by_overlap(&o_rect, &c_rect, 0.8);
            let same_col = geom::same_col_by_overlap(&o_rect, &c_rect, 0.8);
            let primary = match dir {
                crate::MoveDir::Right => {
                    if c_rect.left() >= o_rect.right() - eps && same_row {
                        Some((c_rect.left() - o_rect.right(), (cy - o_cy).abs()))
                    } else {
                        None
                    }
                }
                crate::MoveDir::Left => {
                    if c_rect.right() <= o_rect.left() + eps && same_row {
                        Some((o_rect.left() - c_rect.right(), (cy - o_cy).abs()))
                    } else {
                        None
                    }
                }
                crate::MoveDir::Up => {
                    if c_rect.top() <= o_rect.bottom() + eps && same_col {
                        Some((o_rect.bottom() - c_rect.top(), (cx - o_cx).abs()))
                    } else {
                        None
                    }
                }
                crate::MoveDir::Down => {
                    if c_rect.bottom() >= o_rect.top() - eps && same_col {
                        Some((c_rect.bottom() - o_rect.top(), (cx - o_cx).abs()))
                    } else {
                        None
                    }
                }
            };

            if let Some((axis_delta, tie)) = primary {
                let best_slot = if w.app == origin.app {
                    &mut primary_best_same
                } else {
                    &mut primary_best_other
                };
                let pref = if id_match { 0 } else { 1 };
                match best_slot {
                    None => *best_slot = Some((pref, axis_delta, tie, w.pid, w.id)),
                    Some((best_pref, best_axis, best_tie, _, _)) => {
                        if pref < *best_pref
                            || (pref == *best_pref
                                && (axis_delta < *best_axis
                                    || (geom::approx_eq_eps(axis_delta, *best_axis, eps)
                                        && tie < *best_tie)))
                        {
                            *best_slot = Some((pref, axis_delta, tie, w.pid, w.id));
                        }
                    }
                }
                tracing::debug!(
                    "FocusDir primary cand pid={} id={} pref={} axis_delta={:.1} tie={:.1}",
                    w.pid,
                    w.id,
                    pref,
                    axis_delta,
                    tie
                );
                continue;
            }

            let dx = cx - o_cx;
            let dy = cy - o_cy;
            let fallback_ok = match dir {
                crate::MoveDir::Right => dx > eps,
                crate::MoveDir::Left => dx < -eps,
                crate::MoveDir::Up => dy < -eps,
                crate::MoveDir::Down => dy > eps,
            };
            if !fallback_ok {
                continue;
            }
            let score = match dir {
                crate::MoveDir::Right | crate::MoveDir::Left => {
                    let bias = if same_row { 0.25 } else { 1.0 };
                    let (primary, secondary) =
                        geom::center_distance_bias(&o_rect, &c_rect, geom::Axis::Horizontal);
                    primary * primary + (bias * secondary) * (bias * secondary)
                }
                crate::MoveDir::Up | crate::MoveDir::Down => {
                    let bias = if same_col { 0.25 } else { 1.0 };
                    let (primary, secondary) =
                        geom::center_distance_bias(&o_rect, &c_rect, geom::Axis::Vertical);
                    primary * primary + (bias * secondary) * (bias * secondary)
                }
            };
            let best_slot = if w.app == origin.app {
                &mut fallback_best_same
            } else {
                &mut fallback_best_other
            };
            let pref = if id_match { 0 } else { 1 };
            match best_slot {
                None => *best_slot = Some((pref, score, w.pid, w.id)),
                Some((best_pref, best, _, _)) => {
                    if pref < *best_pref || (pref == *best_pref && score < *best) {
                        *best_slot = Some((pref, score, w.pid, w.id));
                    }
                }
            }
            tracing::debug!(
                "FocusDir fallback cand pid={} id={} pref={} score={:.1}",
                w.pid,
                w.id,
                pref,
                score
            );
        }

        let target = if let Some((_pref, _d, _t, pid, id)) = primary_best_same {
            Some((pid, id))
        } else if let Some((_pref, _d, _t, pid, id)) = primary_best_other {
            Some((pid, id))
        } else if let Some((_pref, _s, pid, id)) = fallback_best_same {
            Some((pid, id))
        } else if let Some((_pref, _s, pid, id)) = fallback_best_other {
            Some((pid, id))
        } else {
            None
        };
        if let Some((pid, id)) = target {
            tracing::info!("FocusDir target pid={} id={}", pid, id);
            let _ = crate::raise::raise_window(pid, id);
            let _ = request_activate_pid(pid);
        }
        Ok(())
    })()
}
