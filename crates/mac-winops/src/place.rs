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

// Stage 4: shrink→move→grow fallback parameters
const FALLBACK_SAFE_MAX_W: f64 = 400.0;
const FALLBACK_SAFE_MAX_H: f64 = 300.0;

/// Logical axis used for corrective nudges.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Axis {
    X,
    Y,
}

#[inline]
fn skip_reason_for_role_subrole(role: &str, subrole: &str) -> Option<&'static str> {
    // Conservative gating: skip common non-movable/transient window types.
    // These are matched against AXRole/AXSubrole values observed in practice.
    // - Sheets: AXRole == "AXSheet"
    // - Popovers: seen as role or subrole depending on host; treat both
    // - Dialogs and system dialogs: subrole markers
    // - Floating tool palettes: not user-movable in the same sense
    let r = role;
    let s = subrole;
    if r == "AXSheet" {
        return Some("role=AXSheet");
    }
    if r == "AXPopover" || s == "AXPopover" {
        return Some("popover");
    }
    if s == "AXDialog" || s == "AXSystemDialog" {
        return Some("dialog");
    }
    if s == "AXFloatingWindow" {
        return Some("floating");
    }
    None
}

/// Options controlling placement attempts and fallback used primarily by tests.
#[derive(Debug, Clone, Copy, Default)]
pub struct PlaceAttemptOptions {
    /// Force a second attempt with size->pos even if the first converged.
    pub force_second_attempt: bool,
    /// Disable size->pos retry; only attempt pos->size.
    pub pos_first_only: bool,
    /// Force shrink->move->grow fallback even if dual-order converged.
    pub force_shrink_move_grow: bool,
}

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
fn one_axis_off(d: (f64, f64, f64, f64), eps: f64) -> Option<Axis> {
    let x_ok = d.0 <= eps && d.2 <= eps; // dx,dw within eps
    let y_ok = d.1 <= eps && d.3 <= eps; // dy,dh within eps
    if x_ok && !y_ok {
        Some(Axis::Y)
    } else if y_ok && !x_ok {
        Some(Axis::X)
    } else {
        None
    }
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
fn log_failure_context(win: &crate::AXElem, role: &str, subrole: &str) {
    let (can_pos, can_size) = crate::ax::ax_settable_pos_size(win.as_ptr());
    let s_pos = match can_pos {
        Some(true) => "true",
        Some(false) => "false",
        None => "unknown",
    };
    let s_size = match can_size {
        Some(true) => "true",
        Some(false) => "false",
        None => "unknown",
    };
    debug!(
        "failure: role='{}' subrole='{}' settable_pos={} settable_size={}",
        role, subrole, s_pos, s_size
    );
}

/// Decide the initial setter ordering based on cached settable flags.
/// Returns `true` for `pos->size`, `false` for `size->pos`.
#[inline]
fn choose_initial_order(can_pos: Option<bool>, can_size: Option<bool>) -> bool {
    match (can_pos, can_size) {
        (Some(false), Some(true)) => false,
        (Some(true), Some(false)) => true,
        (Some(true), Some(true)) => true,
        _ => true,
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

#[inline]
fn guard_bad_coord_space(target: &Rect, vf_x: f64, vf_y: f64) -> Result<()> {
    // Trigger only when target is near global origin AND the chosen screen's
    // origin is not at the global origin (i.e., likely a non‑primary screen).
    let near_zero =
        geom::approx_eq(target.x, 0.0, VERIFY_EPS) && geom::approx_eq(target.y, 0.0, VERIFY_EPS);
    let non_primary =
        !geom::approx_eq(vf_x, 0.0, VERIFY_EPS) || !geom::approx_eq(vf_y, 0.0, VERIFY_EPS);
    if near_zero && non_primary {
        debug!(
            "coordspace: guard hit — target=({:.1},{:.1},{:.1},{:.1}) vf_origin=({:.1},{:.1}); failing with BadCoordinateSpace",
            target.x, target.y, target.w, target.h, vf_x, vf_y
        );
        return Err(Error::BadCoordinateSpace);
    }
    Ok(())
}

/// Stage 4: Fallback sequence to avoid edge clamps when growing while moving.
/// 1) Shrink to a safe size (<= 400x300) at current position.
/// 2) Move to the final position using pos->size ordering (position first).
/// 3) Grow to the final size using size->pos ordering (size first).
fn fallback_shrink_move_grow(
    op_label: &str,
    win: &crate::AXElem,
    attr_pos: core_foundation::string::CFStringRef,
    attr_size: core_foundation::string::CFStringRef,
    target: &Rect,
) -> Result<Rect> {
    // Determine safe size bounded by constants and no larger than target.
    let safe_w = target.w.min(FALLBACK_SAFE_MAX_W);
    let safe_h = target.h.min(FALLBACK_SAFE_MAX_H);

    // Read current position for initial shrink step.
    let cur_p = ax_get_point(win.as_ptr(), attr_pos)?;
    let cur_s = ax_get_size(win.as_ptr(), attr_size)?;
    let _ = cur_s; // silence in case logs change

    // Step 1: shrink at current position (size then pos ordering).
    let shrink_rect = Rect {
        x: cur_p.x,
        y: cur_p.y,
        w: safe_w,
        h: safe_h,
    };
    debug!(
        "fallback:shrink -> ({:.1},{:.1},{:.1},{:.1})",
        shrink_rect.x, shrink_rect.y, shrink_rect.w, shrink_rect.h
    );
    let (_got_shrink, _settle_ms_shrink) = apply_and_wait(
        op_label,
        win,
        attr_pos,
        attr_size,
        &shrink_rect,
        false, // size then pos
        VERIFY_EPS,
    )?;

    // Step 2: move to final position with safe size (position then size).
    let move_rect = Rect {
        x: target.x,
        y: target.y,
        w: safe_w,
        h: safe_h,
    };
    debug!(
        "fallback:move -> ({:.1},{:.1},{:.1},{:.1})",
        move_rect.x, move_rect.y, move_rect.w, move_rect.h
    );
    let (_got_move, _settle_ms_move) = apply_and_wait(
        op_label, win, attr_pos, attr_size, &move_rect, true, // pos then size
        VERIFY_EPS,
    )?;

    // Step 3: grow to final size at final position (size then pos).
    debug!(
        "fallback:grow -> ({:.1},{:.1},{:.1},{:.1})",
        target.x, target.y, target.w, target.h
    );
    let (got_grow, _settle_ms_grow) = apply_and_wait(
        op_label, win, attr_pos, attr_size, target, false, // size then pos
        VERIFY_EPS,
    )?;

    Ok(got_grow)
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
    let (can_pos, can_size) = crate::ax::ax_settable_pos_size(win.as_ptr());
    let do_pos = can_pos != Some(false);
    let do_size = can_size != Some(false);

    if pos_first {
        if do_pos {
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
        } else {
            debug!("skip:set pos (AXPosition not settable)");
        }
        if do_pos && do_size {
            sleep_ms(APPLY_STUTTER_MS);
        }
        if do_size {
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
            debug!("skip:set size (AXSize not settable)");
        }
    } else {
        if do_size {
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
            debug!("skip:set size (AXSize not settable)");
        }
        if do_pos && do_size {
            sleep_ms(APPLY_STUTTER_MS);
        }
        if do_pos {
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
        } else {
            debug!("skip:set pos (AXPosition not settable)");
        }
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

/// Stage 7.1: If only one axis is off, nudge just that axis by re‑applying
/// position on that axis only, then poll for settle. This avoids triggering
/// the full dual‑order or shrink→move→grow sequences when a simple edge clamp
/// caused a coupled dy/dh (or dx/dw) difference.
fn nudge_axis_pos_and_wait(
    _op_label: &str,
    win: &crate::AXElem,
    attr_pos: core_foundation::string::CFStringRef,
    _attr_size: core_foundation::string::CFStringRef,
    target: &Rect,
    axis: Axis,
    eps: f64,
) -> Result<(Rect, u64)> {
    let start = std::time::Instant::now();
    // Read current position/size to construct a single‑axis position write.
    let cur_p = ax_get_point(win.as_ptr(), attr_pos)?;
    let _cur_s = ax_get_size(win.as_ptr(), cfstr("AXSize"))?;
    let new_p = match axis {
        Axis::X => geom::CGPoint {
            x: target.x,
            y: cur_p.y,
        },
        Axis::Y => geom::CGPoint {
            x: cur_p.x,
            y: target.y,
        },
    };
    debug!(
        "axis_nudge: {}: pos -> ({:.1},{:.1})",
        match axis {
            Axis::X => "x",
            Axis::Y => "y",
        },
        new_p.x,
        new_p.y
    );
    let _ = ax_set_point(win.as_ptr(), attr_pos, new_p);

    // Poll for settle or timeout using the same cadence as apply_and_wait.
    let mut waited = 0u64;
    let mut last: Rect;
    loop {
        let p = ax_get_point(win.as_ptr(), attr_pos)?;
        let s = ax_get_size(win.as_ptr(), cfstr("AXSize"))?;
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
        // convert to global coordinates explicitly. This is a no‑op for our
        // current math but guarantees correct space when future callers pass
        // screen‑local values.
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
        // Stage 5 guard: detect (0,0) targets on non‑primary screens
        guard_bad_coord_space(&target, vf_x, vf_y)?;
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
        // Stage 2: choose initial order from cached settable bits; if that
        // does not converge within eps, retry with the opposite order (Stage 3).
        let force_second = false;
        let pos_first_only = false;
        let (can_pos, can_size) = crate::ax::ax_settable_pos_size(win.as_ptr());
        let initial_pos_first = choose_initial_order(can_pos, can_size);
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
        debug!("clamp={}", clamp_flags(&got1, &vf_rect, VERIFY_EPS));
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
                return Err(Error::PlacementVerificationFailed {
                    op: "place_grid",
                    expected: target,
                    got: got1,
                    epsilon: VERIFY_EPS,
                    dx: d1.0,
                    dy: d1.1,
                    dw: d1.2,
                    dh: d1.3,
                });
            }
            // Stage 7.1: If only one axis is off, try a single-axis nudge first.
            let mut attempt_idx = 2u32;
            if let Some(axis) = one_axis_off(d1, VERIFY_EPS) {
                let (got_ax, _settle_ms_ax) = nudge_axis_pos_and_wait(
                    "place_grid",
                    &win,
                    attr_pos,
                    attr_size,
                    &target,
                    axis,
                    VERIFY_EPS,
                )?;
                let dax = diffs(&got_ax, &target);
                debug!("clamp={}", clamp_flags(&got_ax, &vf_rect, VERIFY_EPS));
                let label = match axis {
                    Axis::X => "axis-pos:x",
                    Axis::Y => "axis-pos:y",
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
            debug!("clamp={}", clamp_flags(&got2, &vf_rect, VERIFY_EPS));
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
                    Err(Error::PlacementVerificationFailed {
                        op: "place_grid",
                        expected: target,
                        got: got3,
                        epsilon: VERIFY_EPS,
                        dx: d3.0,
                        dy: d3.1,
                        dw: d3.2,
                        dh: d3.3,
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
                    Err(Error::PlacementVerificationFailed {
                        op: "place_grid",
                        expected: target,
                        got: got3,
                        epsilon: VERIFY_EPS,
                        dx: d3.0,
                        dy: d3.1,
                        dw: d3.2,
                        dh: d3.3,
                    })
                }
            }
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
        guard_bad_coord_space(&target, vf_x, vf_y)?;
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
        // Stage 2: choose initial order from cached settable bits; if that
        // does not converge within eps, retry with the opposite order (Stage 3).
        let force_second = false;
        let pos_first_only = false;
        let (can_pos, can_size) = crate::ax::ax_settable_pos_size(win.as_ptr());
        let initial_pos_first = choose_initial_order(can_pos, can_size);
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
            "place_grid_focused",
            &win,
            attr_pos,
            attr_size,
            &target,
            initial_pos_first,
            VERIFY_EPS,
        )?;
        let d1 = diffs(&got1, &target);
        debug!("clamp={}", clamp_flags(&got1, &vf_rect, VERIFY_EPS));
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
                "WinOps: place_grid_focused verified | pid={} target=({:.1},{:.1},{:.1},{:.1}) got=({:.1},{:.1},{:.1},{:.1}) diff=(dx={:.2},dy={:.2},dw={:.2},dh={:.2})",
                pid,
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
                return Err(Error::PlacementVerificationFailed {
                    op: "place_grid_focused",
                    expected: target,
                    got: got1,
                    epsilon: VERIFY_EPS,
                    dx: d1.0,
                    dy: d1.1,
                    dw: d1.2,
                    dh: d1.3,
                });
            }
            // Stage 7.1: If only one axis is off, try a single-axis nudge first.
            let mut attempt_idx = 2u32;
            if let Some(axis) = one_axis_off(d1, VERIFY_EPS) {
                let (got_ax, _settle_ms_ax) = nudge_axis_pos_and_wait(
                    "place_grid_focused",
                    &win,
                    attr_pos,
                    attr_size,
                    &target,
                    axis,
                    VERIFY_EPS,
                )?;
                let dax = diffs(&got_ax, &target);
                debug!("clamp={}", clamp_flags(&got_ax, &vf_rect, VERIFY_EPS));
                let label = match axis {
                    Axis::X => "axis-pos:x",
                    Axis::Y => "axis-pos:y",
                };
                log_summary(label, attempt_idx, VERIFY_EPS, dax);
                if within_eps(dax, VERIFY_EPS) {
                    debug!("verified=true");
                    debug!("order_used=axis-pos, attempts=2");
                    debug!(
                        "WinOps: place_grid_focused verified | pid={} target=({:.1},{:.1},{:.1},{:.1}) got=({:.1},{:.1},{:.1},{:.1}) diff=(dx={:.2},dy={:.2},dw={:.2},dh={:.2})",
                        pid,
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
            // Stage 3: retry with the opposite order
            let (got2, _settle_ms2) = apply_and_wait(
                "place_grid_focused",
                &win,
                attr_pos,
                attr_size,
                &target,
                !initial_pos_first,
                VERIFY_EPS,
            )?;
            let d2 = diffs(&got2, &target);
            debug!("clamp={}", clamp_flags(&got2, &vf_rect, VERIFY_EPS));
            log_summary(
                if initial_pos_first {
                    "size->pos"
                } else {
                    "pos->size"
                },
                attempt_idx,
                VERIFY_EPS,
                d2,
            );
            let force_smg = false;
            if force_smg {
                debug!("fallback_used=true");
                let got3 = fallback_shrink_move_grow(
                    "place_grid_focused",
                    &win,
                    attr_pos,
                    attr_size,
                    &target,
                )?;
                let d3 = diffs(&got3, &target);
                if within_eps(d3, VERIFY_EPS) {
                    debug!("verified=true");
                    debug!("order_used=shrink->move->grow, attempts=3");
                    debug!(
                        "WinOps: place_grid_focused verified | pid={} target=({:.1},{:.1},{:.1},{:.1}) got=({:.1},{:.1},{:.1},{:.1}) diff=(dx={:.2},dy={:.2},dw={:.2},dh={:.2})",
                        pid,
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
                    Err(Error::PlacementVerificationFailed {
                        op: "place_grid_focused",
                        expected: target,
                        got: got3,
                        epsilon: VERIFY_EPS,
                        dx: d3.0,
                        dy: d3.1,
                        dw: d3.2,
                        dh: d3.3,
                    })
                }
            } else if within_eps(d2, VERIFY_EPS) {
                debug!("verified=true");
                debug!("order_used=size->pos, attempts=2");
                debug!(
                    "WinOps: place_grid_focused verified | pid={} target=({:.1},{:.1},{:.1},{:.1}) got=({:.1},{:.1},{:.1},{:.1}) diff=(dx={:.2},dy={:.2},dw={:.2},dh={:.2})",
                    pid,
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
                let got3 = fallback_shrink_move_grow(
                    "place_grid_focused",
                    &win,
                    attr_pos,
                    attr_size,
                    &target,
                )?;
                let d3 = diffs(&got3, &target);
                if within_eps(d3, VERIFY_EPS) {
                    debug!("verified=true");
                    debug!("order_used=shrink->move->grow, attempts=3");
                    debug!(
                        "WinOps: place_grid_focused verified | pid={} target=({:.1},{:.1},{:.1},{:.1}) got=({:.1},{:.1},{:.1},{:.1}) diff=(dx={:.2},dy={:.2},dw={:.2},dh={:.2})",
                        pid,
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
                    Err(Error::PlacementVerificationFailed {
                        op: "place_grid_focused",
                        expected: target,
                        got: got3,
                        epsilon: VERIFY_EPS,
                        dx: d3.0,
                        dy: d3.1,
                        dw: d3.2,
                        dh: d3.3,
                    })
                }
            }
        }
    })()
}

/// As `place_grid_focused` but with explicit attempt options (smoketests).
pub fn place_grid_focused_opts(
    pid: i32,
    cols: u32,
    rows: u32,
    col: u32,
    row: u32,
    opts: PlaceAttemptOptions,
) -> Result<()> {
    ax_check()?;
    let mtm = MainThreadMarker::new().ok_or(Error::MainThread)?;
    let win = crate::focused_window_for_pid(pid)?;
    let attr_pos = cfstr("AXPosition");
    let attr_size = cfstr("AXSize");

    (|| -> Result<()> {
        normalize_before_move(&win, pid, None)?;
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
        let col = core::cmp::min(col, cols.saturating_sub(1));
        let row = core::cmp::min(row, rows.saturating_sub(1));
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
            "WinOps: place_grid_focused_opts: pid={} role='{}' subrole='{}' title='{}' cols={} rows={} col={} row={} | cur=({:.1},{:.1},{:.1},{:.1}) vf=({:.1},{:.1},{:.1},{:.1}) target=({:.1},{:.1},{:.1},{:.1})",
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
        let force_second = opts.force_second_attempt;
        let pos_first_only = opts.pos_first_only;
        if force_second {
            debug!("opts: force_second_attempt=true");
        }
        if pos_first_only {
            debug!("opts: pos_first_only=true");
        }
        let (can_pos, can_size) = crate::ax::ax_settable_pos_size(win.as_ptr());
        let initial_pos_first = choose_initial_order(can_pos, can_size);
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
            "place_grid_focused",
            &win,
            attr_pos,
            attr_size,
            &target,
            initial_pos_first,
            VERIFY_EPS,
        )?;
        let d1 = diffs(&got1, &target);
        debug!("clamp={}", clamp_flags(&got1, &vf_rect, VERIFY_EPS));
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
                "WinOps: place_grid_focused verified | pid={} target=({:.1},{:.1},{:.1},{:.1}) got=({:.1},{:.1},{:.1},{:.1}) diff=(dx={:.2},dy={:.2},dw={:.2},dh={:.2})",
                pid,
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
                return Err(Error::PlacementVerificationFailed {
                    op: "place_grid_focused",
                    expected: target,
                    got: got1,
                    epsilon: VERIFY_EPS,
                    dx: d1.0,
                    dy: d1.1,
                    dw: d1.2,
                    dh: d1.3,
                });
            }
            // Stage 7.1: If only one axis is off, try a single-axis nudge first.
            let mut attempt_idx = 2u32;
            if let Some(axis) = one_axis_off(d1, VERIFY_EPS) {
                let (got_ax, _settle_ms_ax) = nudge_axis_pos_and_wait(
                    "place_grid_focused",
                    &win,
                    attr_pos,
                    attr_size,
                    &target,
                    axis,
                    VERIFY_EPS,
                )?;
                let dax = diffs(&got_ax, &target);
                debug!("clamp={}", clamp_flags(&got_ax, &vf_rect, VERIFY_EPS));
                let label = match axis {
                    Axis::X => "axis-pos:x",
                    Axis::Y => "axis-pos:y",
                };
                log_summary(label, attempt_idx, VERIFY_EPS, dax);
                if within_eps(dax, VERIFY_EPS) {
                    debug!("verified=true");
                    debug!("order_used=axis-pos, attempts=2");
                    debug!(
                        "WinOps: place_grid_focused verified | pid={} target=({:.1},{:.1},{:.1},{:.1}) got=({:.1},{:.1},{:.1},{:.1}) diff=(dx={:.2},dy={:.2},dw={:.2},dh={:.2})",
                        pid,
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
            let (got2, _settle_ms2) = apply_and_wait(
                "place_grid_focused",
                &win,
                attr_pos,
                attr_size,
                &target,
                !initial_pos_first,
                VERIFY_EPS,
            )?;
            let d2 = diffs(&got2, &target);
            debug!("clamp={}", clamp_flags(&got2, &vf_rect, VERIFY_EPS));
            log_summary(
                if initial_pos_first {
                    "size->pos"
                } else {
                    "pos->size"
                },
                attempt_idx,
                VERIFY_EPS,
                d2,
            );
            let force_smg = opts.force_shrink_move_grow;
            if force_smg {
                debug!("opts: force_shrink_move_grow=true");
                debug!("fallback_used=true");
                let got3 = fallback_shrink_move_grow(
                    "place_grid_focused",
                    &win,
                    attr_pos,
                    attr_size,
                    &target,
                )?;
                let d3 = diffs(&got3, &target);
                if within_eps(d3, VERIFY_EPS) {
                    debug!("verified=true");
                    debug!("order_used=shrink->move->grow, attempts=3");
                    debug!(
                        "WinOps: place_grid_focused verified | pid={} target=({:.1},{:.1},{:.1},{:.1}) got=({:.1},{:.1},{:.1},{:.1}) diff=(dx={:.2},dy={:.2},dw={:.2},dh={:.2})",
                        pid,
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
                    Err(Error::PlacementVerificationFailed {
                        op: "place_grid_focused",
                        expected: target,
                        got: got3,
                        epsilon: VERIFY_EPS,
                        dx: d3.0,
                        dy: d3.1,
                        dw: d3.2,
                        dh: d3.3,
                    })
                }
            } else if within_eps(d2, VERIFY_EPS) {
                debug!("verified=true");
                debug!("order_used=size->pos, attempts=2");
                debug!(
                    "WinOps: place_grid_focused verified | pid={} target=({:.1},{:.1},{:.1},{:.1}) got=({:.1},{:.1},{:.1},{:.1}) diff=(dx={:.2},dy={:.2},dw={:.2},dh={:.2})",
                    pid,
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
                debug!("fallback_used=true");
                let got3 = fallback_shrink_move_grow(
                    "place_grid_focused",
                    &win,
                    attr_pos,
                    attr_size,
                    &target,
                )?;
                let d3 = diffs(&got3, &target);
                if within_eps(d3, VERIFY_EPS) {
                    debug!("verified=true");
                    debug!("order_used=shrink->move->grow, attempts=3");
                    debug!(
                        "WinOps: place_grid_focused verified | pid={} target=({:.1},{:.1},{:.1},{:.1}) got=({:.1},{:.1},{:.1},{:.1}) diff=(dx={:.2},dy={:.2},dw={:.2},dh={:.2})",
                        pid,
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
                    Err(Error::PlacementVerificationFailed {
                        op: "place_grid_focused",
                        expected: target,
                        got: got3,
                        epsilon: VERIFY_EPS,
                        dx: d3.0,
                        dy: d3.1,
                        dw: d3.2,
                        dh: d3.3,
                    })
                }
            }
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
        guard_bad_coord_space(&target, vf_x, vf_y)?;
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

        // Stage 2: choose initial order from cached settable bits; if that
        // does not converge within eps, retry with the opposite order (Stage 3).
        let force_second = false;
        let pos_first_only = false;
        let (can_pos, can_size) = crate::ax::ax_settable_pos_size(win.as_ptr());
        let initial_pos_first = choose_initial_order(can_pos, can_size);
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
            "place_move_grid",
            &win,
            attr_pos,
            attr_size,
            &target,
            initial_pos_first,
            VERIFY_EPS,
        )?;
        let d1 = diffs(&got1, &target);
        debug!("clamp={}", clamp_flags(&got1, &vf_rect, VERIFY_EPS));
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
                "WinOps: place_move_grid verified | id={} target=({:.1},{:.1},{:.1},{:.1}) got=({:.1},{:.1},{:.1},{:.1}) diff=(dx={:.2},dy={:.2},dw={:.2},dh={:.2})",
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
                return Err(Error::PlacementVerificationFailed {
                    op: "place_move_grid",
                    expected: target,
                    got: got1,
                    epsilon: VERIFY_EPS,
                    dx: d1.0,
                    dy: d1.1,
                    dw: d1.2,
                    dh: d1.3,
                });
            }
            // Stage 7.1: If only one axis is off, try a single-axis nudge first.
            let mut attempt_idx = 2u32;
            if let Some(axis) = one_axis_off(d1, VERIFY_EPS) {
                let (got_ax, _settle_ms_ax) = nudge_axis_pos_and_wait(
                    "place_move_grid",
                    &win,
                    attr_pos,
                    attr_size,
                    &target,
                    axis,
                    VERIFY_EPS,
                )?;
                let dax = diffs(&got_ax, &target);
                debug!("clamp={}", clamp_flags(&got_ax, &vf_rect, VERIFY_EPS));
                let label = match axis {
                    Axis::X => "axis-pos:x",
                    Axis::Y => "axis-pos:y",
                };
                log_summary(label, attempt_idx, VERIFY_EPS, dax);
                if within_eps(dax, VERIFY_EPS) {
                    debug!("verified=true");
                    debug!("order_used=axis-pos, attempts=2");
                    debug!(
                        "WinOps: place_move_grid verified | id={} target=({:.1},{:.1},{:.1},{:.1}) got=({:.1},{:.1},{:.1},{:.1}) diff=(dx={:.2},dy={:.2},dw={:.2},dh={:.2})",
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
            // Stage 3: retry with the opposite order
            let (got2, _settle_ms2) = apply_and_wait(
                "place_move_grid",
                &win,
                attr_pos,
                attr_size,
                &target,
                !initial_pos_first,
                VERIFY_EPS,
            )?;
            let d2 = diffs(&got2, &target);
            debug!("clamp={}", clamp_flags(&got2, &vf_rect, VERIFY_EPS));
            log_summary(
                if initial_pos_first {
                    "size->pos"
                } else {
                    "pos->size"
                },
                attempt_idx,
                VERIFY_EPS,
                d2,
            );
            let force_smg = false;
            if force_smg {
                debug!("fallback_used=true");
                let got3 = fallback_shrink_move_grow(
                    "place_move_grid",
                    &win,
                    attr_pos,
                    attr_size,
                    &target,
                )?;
                let d3 = diffs(&got3, &target);
                if within_eps(d3, VERIFY_EPS) {
                    debug!("verified=true");
                    debug!("order_used=shrink->move->grow, attempts=3");
                    debug!(
                        "WinOps: place_move_grid verified | id={} target=({:.1},{:.1},{:.1},{:.1}) got=({:.1},{:.1},{:.1},{:.1}) diff=(dx={:.2},dy={:.2},dw={:.2},dh={:.2})",
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
                    Err(Error::PlacementVerificationFailed {
                        op: "place_move_grid",
                        expected: target,
                        got: got3,
                        epsilon: VERIFY_EPS,
                        dx: d3.0,
                        dy: d3.1,
                        dw: d3.2,
                        dh: d3.3,
                    })
                }
            } else if within_eps(d2, VERIFY_EPS) {
                debug!("verified=true");
                debug!("order_used=size->pos, attempts=2");
                debug!(
                    "WinOps: place_move_grid verified | id={} target=({:.1},{:.1},{:.1},{:.1}) got=({:.1},{:.1},{:.1},{:.1}) diff=(dx={:.2},dy={:.2},dw={:.2},dh={:.2})",
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
                let got3 = fallback_shrink_move_grow(
                    "place_move_grid",
                    &win,
                    attr_pos,
                    attr_size,
                    &target,
                )?;
                let d3 = diffs(&got3, &target);
                if within_eps(d3, VERIFY_EPS) {
                    debug!("verified=true");
                    debug!("order_used=shrink->move->grow, attempts=3");
                    debug!(
                        "WinOps: place_move_grid verified | id={} target=({:.1},{:.1},{:.1},{:.1}) got=({:.1},{:.1},{:.1},{:.1}) diff=(dx={:.2},dy={:.2},dw={:.2},dh={:.2})",
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
                    Err(Error::PlacementVerificationFailed {
                        op: "place_move_grid",
                        expected: target,
                        got: got3,
                        epsilon: VERIFY_EPS,
                        dx: d3.0,
                        dy: d3.1,
                        dw: d3.2,
                        dh: d3.3,
                    })
                }
            }
        }
    })()
}

#[cfg(test)]
mod tests {
    use super::choose_initial_order;

    #[test]
    fn order_both_settable_defaults_pos_first() {
        assert!(choose_initial_order(Some(true), Some(true)));
    }

    #[test]
    fn order_size_only_prefers_size_first() {
        assert!(!choose_initial_order(Some(false), Some(true)));
    }

    #[test]
    fn order_pos_only_prefers_pos_first() {
        assert!(choose_initial_order(Some(true), Some(false)));
    }

    #[test]
    fn order_unknown_defaults_pos_first() {
        assert!(choose_initial_order(None, None));
        assert!(choose_initial_order(Some(true), None));
        assert!(choose_initial_order(None, Some(true)));
    }
}
