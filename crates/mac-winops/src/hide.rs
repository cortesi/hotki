use std::ffi::c_void;

use core_foundation::{
    array::{CFArray, CFArrayGetCount, CFArrayGetValueAtIndex},
    base::{CFRelease, CFTypeRef, TCFType},
};
use tracing::debug;

pub(crate) use crate::AXElem;
use crate::{
    Error, Result, ScreenCorner,
    ax::{
        AXUIElementCopyAttributeValue, AXUIElementCreateApplication, ax_check, ax_get_point,
        ax_get_size, ax_get_string, ax_set_point, ax_set_size, cfstr,
    },
    frame_storage::{HIDDEN_FRAMES, HIDDEN_FRAMES_CAP},
    geom::{self, CGPoint},
    request_activate_pid,
};

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRetain(cf: CFTypeRef) -> CFTypeRef;
}

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
    let win_raw: *mut c_void = unsafe {
        let app = AXUIElementCreateApplication(pid);
        if app.is_null() {
            return Err(Error::AppElement);
        }
        let mut wins_ref: CFTypeRef = std::ptr::null_mut();
        let err = AXUIElementCopyAttributeValue(app, cfstr("AXWindows"), &mut wins_ref);
        if err != 0 || wins_ref.is_null() {
            CFRelease(app as CFTypeRef);
            return Err(Error::FocusedWindow);
        }
        let arr = CFArray::<*const c_void>::wrap_under_create_rule(wins_ref as _);
        let n = CFArrayGetCount(arr.as_concrete_TypeRef());
        let mut chosen: *mut c_void = std::ptr::null_mut();
        for i in 0..n {
            let w = CFArrayGetValueAtIndex(arr.as_concrete_TypeRef(), i) as *mut c_void;
            if w.is_null() {
                continue;
            }
            let role = ax_get_string(w, cfstr("AXRole")).unwrap_or_default();
            if role == "AXWindow" {
                chosen = w;
                break;
            }
        }
        if !chosen.is_null() {
            CFRetain(chosen as CFTypeRef);
        }
        CFRelease(app as CFTypeRef);
        if chosen.is_null() {
            return Err(Error::FocusedWindow);
        }
        chosen
    };
    let win = AXElem::new(win_raw);
    let attr_pos = cfstr("AXPosition");
    let attr_size = cfstr("AXSize");
    let cur_p = ax_get_point(win.as_ptr(), attr_pos)?;
    let cur_s = ax_get_size(win.as_ptr(), attr_size)?;

    let key = crate::frontmost_window_for_pid(pid).map(|w| (pid, w.id));
    let is_hidden = key
        .as_ref()
        .map(|k| HIDDEN_FRAMES.lock().contains_key(k))
        .unwrap_or(false);
    let do_hide = match desired {
        crate::Desired::On => true,
        crate::Desired::Off => false,
        crate::Desired::Toggle => !is_hidden,
    };

    if do_hide {
        if let Some(k) = key {
            let mut map = HIDDEN_FRAMES.lock();
            if map.len() >= HIDDEN_FRAMES_CAP
                && let Some(old_k) = map.keys().next().cloned()
            {
                let _ = map.remove(&old_k);
            }
            map.insert(k, (cur_p, cur_s));
        }

        // Large overshoot; rely on WindowServer clamping for tight placement
        let tgt = geom::overshoot_target(cur_p, corner, 100_000.0);
        let (tx, ty) = (tgt.x, tgt.y);

        let _ = ax_set_size(win.as_ptr(), attr_size, cur_s);
        ax_set_point(win.as_ptr(), attr_pos, CGPoint { x: tx, y: ty })?;

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
                CGPoint {
                    x: nudge_x,
                    y: cur_p.y,
                },
            );
            let _ = ax_set_point(win.as_ptr(), attr_pos, CGPoint { x: tx, y: ty });
        }

        // Tightening pass
        if let Ok(mut best) = ax_get_point(win.as_ptr(), attr_pos) {
            let mut step = 128.0;
            for _ in 0..3 {
                // X axis drive
                let mut local = step;
                while local >= 1.0 {
                    let cand = match corner {
                        ScreenCorner::BottomRight => CGPoint {
                            x: best.x + local,
                            y: best.y,
                        },
                        ScreenCorner::BottomLeft | ScreenCorner::TopLeft => CGPoint {
                            x: best.x - local,
                            y: best.y,
                        },
                    };
                    let _ = ax_set_point(win.as_ptr(), attr_pos, cand);
                    if let Ok(np) = ax_get_point(win.as_ptr(), attr_pos) {
                        let improved = match corner {
                            ScreenCorner::BottomRight => np.x > best.x + 0.5,
                            ScreenCorner::BottomLeft | ScreenCorner::TopLeft => np.x < best.x - 0.5,
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
                        ScreenCorner::BottomRight | ScreenCorner::BottomLeft => CGPoint {
                            x: best.x,
                            y: best.y + local,
                        },
                        ScreenCorner::TopLeft => CGPoint {
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
        }
    } else if let Some(k) = key {
        let mut map = HIDDEN_FRAMES.lock();
        if let Some((p, s)) = map.remove(&k) {
            let _ = ax_set_size(win.as_ptr(), attr_size, s);
            let _ = ax_set_point(win.as_ptr(), attr_pos, p);
        }
    }

    Ok(())
}

/// Hide or reveal the focused window at the bottom-left corner of the screen.
pub fn hide_bottom_left(pid: i32, desired: crate::Desired) -> Result<()> {
    hide_corner(pid, desired, ScreenCorner::BottomLeft)
}
