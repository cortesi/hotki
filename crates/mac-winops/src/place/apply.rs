use tracing::debug;

use super::common::{diffs, now_ms, sleep_ms, within_eps};
use crate::geom::Axis;
use crate::{
    ax::{
        ax_element_pid, ax_get_point, ax_get_size, ax_set_point, ax_set_size, warn_once_nonsettable,
    },
    geom::{self, CGPoint, CGSize, Rect},
};

// Stage 2: settle/polling parameters for apply_and_wait
const APPLY_STUTTER_MS: u64 = 2; // tiny delay between A and B sets
const SETTLE_SLEEP_MS: u64 = 20; // poll cadence while waiting to settle
// For apps that apply geometry asynchronously (e.g., via animations or delayed
// setFrame), allow a longer per-attempt settle window. The overall placement
// still caps attempts elsewhere.
const SETTLE_TOTAL_MS: u64 = 600; // max settle time per attempt

/// Apply position and size in the requested order, then poll until either the
/// window settles within `eps` of `target` or timeout. Returns the last
/// observed rect and the measured settle time in milliseconds.
pub(super) fn apply_and_wait(
    op_label: &str,
    win: &crate::AXElem,
    attr_pos: core_foundation::string::CFStringRef,
    attr_size: core_foundation::string::CFStringRef,
    target: &Rect,
    pos_first: bool,
    eps: f64,
) -> crate::Result<(Rect, u64)> {
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
            if let Some(pid) = ax_element_pid(win.as_ptr()) {
                warn_once_nonsettable(pid, can_pos, can_size);
            }
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
            if let Some(pid) = ax_element_pid(win.as_ptr()) {
                warn_once_nonsettable(pid, can_pos, can_size);
            }
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
            if let Some(pid) = ax_element_pid(win.as_ptr()) {
                warn_once_nonsettable(pid, can_pos, can_size);
            }
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
            if let Some(pid) = ax_element_pid(win.as_ptr()) {
                warn_once_nonsettable(pid, can_pos, can_size);
            }
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

/// Apply size only (do not touch position), then poll until settle or timeout.
pub(super) fn apply_size_only_and_wait(
    op_label: &str,
    win: &crate::AXElem,
    attr_size: core_foundation::string::CFStringRef,
    target_size: (f64, f64),
    eps: f64,
) -> crate::Result<(Rect, u64)> {
    let start = std::time::Instant::now();
    let (w, h) = target_size;
    debug!("WinOps: {} set size-only -> ({:.1},{:.1})", op_label, w, h);
    ax_set_size(
        win.as_ptr(),
        attr_size,
        CGSize {
            width: w,
            height: h,
        },
    )?;
    // Poll identical to apply_and_wait
    let mut waited = 0u64;
    let mut last: Rect;
    loop {
        let p = ax_get_point(win.as_ptr(), crate::ax::cfstr("AXPosition"))?;
        let s = ax_get_size(win.as_ptr(), attr_size)?;
        last = Rect {
            x: p.x,
            y: p.y,
            w: s.width,
            h: s.height,
        };
        let d = ((last.w - w).abs(), (last.h - h).abs());
        if d.0 <= eps && d.1 <= eps {
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
/// position on that axis only, then poll for settle.
pub(super) fn nudge_axis_pos_and_wait(
    _op_label: &str,
    win: &crate::AXElem,
    attr_pos: core_foundation::string::CFStringRef,
    _attr_size: core_foundation::string::CFStringRef,
    target: &Rect,
    axis: Axis,
    eps: f64,
) -> crate::Result<(Rect, u64)> {
    let start = std::time::Instant::now();
    // Read current position/size to construct a single‑axis position write.
    let cur_p = ax_get_point(win.as_ptr(), attr_pos)?;
    let _cur_s = ax_get_size(win.as_ptr(), crate::ax::cfstr("AXSize"))?;
    let new_p = match axis {
        Axis::Horizontal => geom::CGPoint {
            x: target.x,
            y: cur_p.y,
        },
        Axis::Vertical => geom::CGPoint {
            x: cur_p.x,
            y: target.y,
        },
    };
    debug!(
        "axis_nudge: {}: pos -> ({:.1},{:.1})",
        match axis {
            Axis::Horizontal => "x",
            Axis::Vertical => "y",
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
        let s = ax_get_size(win.as_ptr(), crate::ax::cfstr("AXSize"))?;
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

/// Anchor the app's legal size by accepting rounded dimensions and aligning
/// the visually important edges to the grid cell.
pub(super) fn anchor_legal_size_and_wait(
    op_label: &str,
    win: &crate::AXElem,
    attr_pos: core_foundation::string::CFStringRef,
    attr_size: core_foundation::string::CFStringRef,
    target: &Rect,
    observed: &Rect,
    cols: u32,
    rows: u32,
    col: u32,
    row: u32,
    eps: f64,
) -> crate::Result<(Rect, Rect, u64)> {
    // Use the last observed legal size and compute a position so that
    // the chosen edges are anchored to the grid cell.
    let w = observed.w.max(1.0);
    let h = observed.h.max(1.0);

    // Horizontal anchoring: default left; last column anchors right
    let x = if col == cols.saturating_sub(1) && cols > 1 {
        target.right() - w // right edge flush
    } else {
        target.x // left edge flush (including single column)
    };

    // Vertical anchoring: default bottom; last row anchors top
    let y = if row == rows.saturating_sub(1) && rows > 1 {
        target.top() - h // top edge flush
    } else {
        target.y // bottom edge flush (including single row)
    };

    let anchored = Rect { x, y, w, h };
    debug!(
        "anchor_legal: target={} observed={} -> anchored={}",
        target, observed, anchored
    );
    // Apply position-first using the anchored rect.
    let (got, settle) = apply_and_wait(op_label, win, attr_pos, attr_size, &anchored, true, eps)?;
    Ok((got, anchored, settle))
}
// AX setters + settle/polling helpers used by placement ops.
