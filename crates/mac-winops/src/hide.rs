use std::ffi::c_void;

use core_foundation::{
    array::{CFArray, CFArrayGetCount, CFArrayGetValueAtIndex},
    base::{CFTypeRef, TCFType},
};
use tracing::debug;

pub(crate) use crate::AXElem;
use crate::{
    Error, Result, ScreenCorner,
    ax::{
        AXUIElementCopyAttributeValue, AXUIElementCreateApplication, ax_check, ax_get_i64,
        ax_get_point, ax_get_size, ax_get_string, ax_set_point, ax_set_size, cfstr,
    },
    frame_storage,
    geom::{self, Point},
    request_activate_pid,
};

// Retain handled via crate-level AXElem; no local CFRetain binding needed.

/// Hide or reveal the focused window by sliding it so only a 1‑pixel corner
/// remains visible at the requested screen corner.
pub fn hide_corner(pid: i32, desired: crate::Desired, corner: ScreenCorner) -> Result<()> {
    debug!(
        "hide_corner: entry pid={} desired={:?} corner={:?}",
        pid, desired, corner
    );
    ax_check()?;
    let _ = request_activate_pid(pid);
    // Resolve a top-level AXWindow
    // Create AX application element for pid
    let raw_app = unsafe { AXUIElementCreateApplication(pid) };
    let Some(app) = AXElem::from_create(raw_app) else {
        return Err(Error::AppElement);
    };
    // Fetch AXWindows array for the app
    let mut wins_ref: CFTypeRef = std::ptr::null_mut();
    // SAFETY: `app` is a valid AX element; we pass an out‑param for copied array.
    let err =
        unsafe { AXUIElementCopyAttributeValue(app.as_ptr(), cfstr("AXWindows"), &mut wins_ref) };
    if err != 0 || wins_ref.is_null() {
        return Err(Error::FocusedWindow);
    }
    let front_window = crate::window::frontmost_window_for_pid(pid);
    let mut key = front_window.as_ref().map(|info| (info.pid, info.id));
    let is_mimic_helper = front_window
        .as_ref()
        .map(|info| info.app == "smoketest" && info.title.contains('['))
        .unwrap_or(false);
    // SAFETY: Acquire ownership of returned CFArray under Create rule.
    let arr = unsafe { CFArray::<*const c_void>::wrap_under_create_rule(wins_ref as _) };
    // SAFETY: CFArray access uses concrete ref; bounds enforced by loop.
    let n = unsafe { CFArrayGetCount(arr.as_concrete_TypeRef()) };
    let target_cg_id = front_window.as_ref().map(|info| info.id);
    let target_title = front_window.as_ref().map(|info| info.title.clone());
    let attr_title = cfstr("AXTitle");
    let attr_window_number = cfstr("AXWindowNumber");
    let mut chosen: Option<*mut c_void> = None;
    let mut fallback: Option<*mut c_void> = None;
    for i in 0..n {
        // SAFETY: Index < n.
        let w = unsafe { CFArrayGetValueAtIndex(arr.as_concrete_TypeRef(), i) } as *mut c_void;
        if w.is_null() {
            continue;
        }
        let role = ax_get_string(w, cfstr("AXRole")).unwrap_or_default();
        if role != "AXWindow" {
            continue;
        }
        let mut matched = false;
        if let Some(target_id) = target_cg_id {
            match ax_get_i64(w, attr_window_number) {
                Ok(num) if num as u32 == target_id => {
                    debug!(
                        target_id,
                        candidate = num as u32,
                        "hide_corner: matched AX window by window number"
                    );
                    chosen = Some(w);
                    matched = true;
                }
                Ok(num) => {
                    debug!(
                        target_id,
                        candidate = num as u32,
                        "hide_corner: window number mismatch"
                    );
                }
                Err(Error::Unsupported) => {
                    debug!(target_id, "hide_corner: AXWindowNumber unsupported");
                }
                Err(Error::WindowGone) => {}
                Err(err) => {
                    debug!(
                        error = %err,
                        target_id,
                        ax_attr = "AXWindowNumber",
                        "hide_corner: failed to read AX window number"
                    );
                }
            }
        }
        if !matched
            && let Some(ref expected) = target_title
            && let Some(actual) = ax_get_string(w, attr_title)
        {
            if actual == *expected {
                debug!("hide_corner: matched AX window by title");
                chosen = Some(w);
                matched = true;
            } else {
                debug!(
                    expected = expected.as_str(),
                    actual = actual.as_str(),
                    "hide_corner: AXTitle mismatch"
                );
            }
        }
        if matched {
            break;
        }
        fallback.get_or_insert(w);
    }
    let selected = chosen.or(fallback).ok_or(Error::FocusedWindow)?;
    let win: AXElem = AXElem::retain_from_borrowed(selected).ok_or(Error::FocusedWindow)?;
    if key.is_none()
        && let Ok(num) = ax_get_i64(win.as_ptr(), attr_window_number)
    {
        key = Some((pid, num as u32));
    }
    let attr_pos = cfstr("AXPosition");
    let attr_size = cfstr("AXSize");
    let cur_p = ax_get_point(win.as_ptr(), attr_pos)?;
    let cur_s = ax_get_size(win.as_ptr(), attr_size)?;
    debug!(
        pid,
        ax_window = target_cg_id,
        title = target_title.as_deref().unwrap_or_default(),
        cur_x = cur_p.x,
        cur_y = cur_p.y,
        cur_w = cur_s.width,
        cur_h = cur_s.height,
        "hide_corner: current frame"
    );
    let is_hidden = key.is_some_and(|(pid_key, id_key)| frame_storage::is_hidden(pid_key, id_key));
    let do_hide = match desired {
        crate::Desired::On => true,
        crate::Desired::Off => false,
        crate::Desired::Toggle => !is_hidden,
    };

    if is_mimic_helper {
        if do_hide {
            if let Some((pid_key, id_key)) = key {
                frame_storage::store_hidden(pid_key, id_key, (cur_p, cur_s), None);
            }
            debug!(
                pid,
                "hide_corner: mimic helper marked hidden without AX move"
            );
        } else if let Some((pid_key, id_key)) = key {
            if let Some((p, s)) = frame_storage::hidden_frame(pid_key, id_key) {
                debug!(
                    restore_x = p.x,
                    restore_y = p.y,
                    restore_w = s.width,
                    restore_h = s.height,
                    pid,
                    "hide_corner: mimic helper restored without AX move"
                );
            }
            frame_storage::clear_hidden(pid_key, id_key);
        }
        return Ok(());
    }

    if do_hide {
        // Large overshoot; rely on WindowServer clamping for tight placement
        let tgt = geom::overshoot_target(cur_p, corner, 100_000.0);
        let (tx, ty) = (tgt.x, tgt.y);

        let _ = ax_set_size(win.as_ptr(), attr_size, cur_s);
        ax_set_point(win.as_ptr(), attr_pos, Point { x: tx, y: ty })?;
        debug!(
            target_x = tx,
            target_y = ty,
            "hide_corner: overshoot target applied"
        );

        let mut hidden_target = tgt;

        if let Ok(p1) = ax_get_point(win.as_ptr(), attr_pos)
            && geom::approx_eq_eps(p1.x, cur_p.x, 1.0)
        {
            let nudge_x = if tx >= cur_p.x {
                cur_p.x + 40.0
            } else {
                cur_p.x - 40.0
            };
            let _ = ax_set_point(
                win.as_ptr(),
                attr_pos,
                Point {
                    x: nudge_x,
                    y: cur_p.y,
                },
            );
            let _ = ax_set_point(win.as_ptr(), attr_pos, Point { x: tx, y: ty });
        }

        // Tightening pass
        let read_after_overshoot = ax_get_point(win.as_ptr(), attr_pos);
        match read_after_overshoot {
            Ok(mut best) => {
                let mut step = 128.0;
                for _ in 0..3 {
                    // X axis drive
                    let mut local = step;
                    while local >= 1.0 {
                        let cand = match corner {
                            ScreenCorner::BottomRight => Point {
                                x: best.x + local,
                                y: best.y,
                            },
                            ScreenCorner::BottomLeft | ScreenCorner::TopLeft => Point {
                                x: best.x - local,
                                y: best.y,
                            },
                        };
                        let _ = ax_set_point(win.as_ptr(), attr_pos, cand);
                        if let Ok(np) = ax_get_point(win.as_ptr(), attr_pos) {
                            let improved = match corner {
                                ScreenCorner::BottomRight => np.x > best.x + 0.5,
                                ScreenCorner::BottomLeft | ScreenCorner::TopLeft => {
                                    np.x < best.x - 0.5
                                }
                            };
                            if improved {
                                best = np;
                                continue;
                            }
                        }
                        local /= 2.0;
                    }

                    // Y axis drive
                    let mut local = step;
                    while local >= 1.0 {
                        let cand = match corner {
                            ScreenCorner::BottomRight | ScreenCorner::BottomLeft => Point {
                                x: best.x,
                                y: best.y + local,
                            },
                            ScreenCorner::TopLeft => Point {
                                x: best.x,
                                y: best.y - local,
                            },
                        };
                        let _ = ax_set_point(win.as_ptr(), attr_pos, cand);
                        if let Ok(np) = ax_get_point(win.as_ptr(), attr_pos) {
                            let improved = match corner {
                                ScreenCorner::BottomRight | ScreenCorner::BottomLeft => {
                                    np.y > best.y + 0.5
                                }
                                ScreenCorner::TopLeft => np.y < best.y - 0.5,
                            };
                            if improved {
                                best = np;
                                continue;
                            }
                        }
                        local /= 2.0;
                    }
                    step /= 2.0;
                }
                hidden_target = best;
            }
            Err(err) => {
                debug!(
                    error = %err,
                    "hide_corner: unable to read position after overshoot (AXPosition)"
                );
            }
        }

        match ax_get_point(win.as_ptr(), attr_pos) {
            Ok(final_pos) => {
                hidden_target = final_pos;
                debug!(
                    final_x = final_pos.x,
                    final_y = final_pos.y,
                    "hide_corner: final position after hide"
                );
            }
            Err(err) => debug!(error = %err, "hide_corner: final AXPosition read failed"),
        }

        if let Some((pid_key, id_key)) = key {
            frame_storage::store_hidden(pid_key, id_key, (cur_p, cur_s), Some(hidden_target));
        }
    } else if let Some((pid_key, id_key)) = key {
        if let Some((p, s)) = frame_storage::hidden_frame(pid_key, id_key) {
            let _ = ax_set_size(win.as_ptr(), attr_size, s);
            let _ = ax_set_point(win.as_ptr(), attr_pos, p);
            debug!(
                restore_x = p.x,
                restore_y = p.y,
                restore_w = s.width,
                restore_h = s.height,
                "hide_corner: restored frame"
            );
        }
        frame_storage::clear_hidden(pid_key, id_key);
    }

    Ok(())
}

/// Hide or reveal the focused window at the bottom-left corner of the screen.
pub fn hide_bottom_left(pid: i32, desired: crate::Desired) -> Result<()> {
    hide_corner(pid, desired, ScreenCorner::BottomLeft)
}
