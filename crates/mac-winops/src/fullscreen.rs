use mac_keycode::{Chord, Key, Modifier};
use objc2_foundation::MainThreadMarker;
use relaykey::RelayKey;
use tracing::debug;

pub(crate) use crate::focused_window_for_pid;
use crate::{
    Desired, Error, Result,
    ax::{
        ax_bool, ax_check, ax_get_point, ax_get_size, ax_set_bool, ax_set_point, ax_set_size, cfstr,
    },
    frame_storage::{PREV_FRAMES, PREV_FRAMES_CAP},
    frontmost_window_for_pid,
    geom::{Point, Rect, Size},
    screen_util::visible_frame_containing_point,
};

/// Toggle or set native full screen (AXFullScreen) for the focused window of `pid`.
///
/// Requires Accessibility permission. If AXFullScreen is not available, the
/// function synthesizes the standard ⌃⌘F shortcut as a fallback.
pub fn fullscreen_native(pid: i32, desired: Desired) -> Result<()> {
    tracing::info!(
        "WinOps: fullscreen_native enter pid={} desired={:?}",
        pid,
        desired
    );
    ax_check()?;
    let win = focused_window_for_pid(pid)?;
    let attr_fullscreen = cfstr("AXFullScreen");
    match ax_bool(win.as_ptr(), attr_fullscreen) {
        Ok(Some(cur)) => {
            let target = match desired {
                Desired::On => true,
                Desired::Off => false,
                Desired::Toggle => !cur,
            };
            if target != cur {
                ax_set_bool(win.as_ptr(), attr_fullscreen, target)?;
            }
            Ok(())
        }
        // If the attribute is missing/unsupported, fall back to keystroke.
        _ => {
            let mut mods = std::collections::HashSet::new();
            mods.insert(Modifier::Control);
            mods.insert(Modifier::Command);
            let chord = Chord {
                key: Key::F,
                modifiers: mods,
            };
            let rk = RelayKey::new();
            rk.key_down(pid, &chord, false);
            rk.key_up(pid, &chord);
            tracing::info!("WinOps: fullscreen_native fallback keystroke sent");
            Ok(())
        }
    }
}

/// Toggle or set non‑native full screen (maximize to visible frame in current Space)
/// for the focused window of `pid`.
///
/// Requires Accessibility permission and must run on the AppKit main thread
/// (uses `NSScreen::visibleFrame`).
pub fn fullscreen_nonnative(pid: i32, desired: Desired) -> Result<()> {
    tracing::info!(
        "WinOps: fullscreen_nonnative enter pid={} desired={:?}",
        pid,
        desired
    );
    ax_check()?;
    let mtm = MainThreadMarker::new().ok_or(Error::MainThread)?;
    let win = focused_window_for_pid(pid)?;
    let attr_pos = cfstr("AXPosition");
    let attr_size = cfstr("AXSize");

    let result = (|| -> Result<()> {
        let cur_p = ax_get_point(win.as_ptr(), attr_pos)?;
        let cur_s = ax_get_size(win.as_ptr(), attr_size)?;

        let vf = visible_frame_containing_point(mtm, cur_p);
        tracing::debug!("WinOps: visible frame {}", vf);
        let target_p = Point { x: vf.x, y: vf.y };
        let target_s = Size {
            width: vf.w,
            height: vf.h,
        };

        let mut prev_key: Option<(i32, crate::WindowId)> = None;
        let is_full = Rect::from((cur_p, cur_s)).approx_eq(&Rect::from((target_p, target_s)), 1.0);
        let do_set_to_full = match desired {
            Desired::On => true,
            Desired::Off => false,
            Desired::Toggle => !is_full,
        };

        if let Some(w) = frontmost_window_for_pid(pid) {
            prev_key = Some((pid, w.id));
        }

        if do_set_to_full {
            tracing::debug!("WinOps: setting to non-native fullscreen frame");
            if let Some(k) = prev_key {
                let mut map = PREV_FRAMES.lock();
                if map.len() >= PREV_FRAMES_CAP
                    && let Some(old_k) = map.keys().next().cloned()
                {
                    let _ = map.remove(&old_k);
                }
                map.entry(k).or_insert((cur_p, cur_s));
            }
            ax_set_point(win.as_ptr(), attr_pos, target_p)?;
            ax_set_size(win.as_ptr(), attr_size, target_s)?;
        } else {
            tracing::debug!("WinOps: restoring from previous frame if any");
            let restored = if let Some(k) = prev_key {
                let mut map = PREV_FRAMES.lock();
                if let Some((p, s)) = map.remove(&k) {
                    if !Rect::from((p, s)).approx_eq(&Rect::from((cur_p, cur_s)), 1.0) {
                        ax_set_point(win.as_ptr(), attr_pos, p)?;
                        ax_set_size(win.as_ptr(), attr_size, s)?;
                    }
                    true
                } else {
                    false
                }
            } else {
                false
            };
            if !restored {
                debug!("no previous frame to restore; non-native Off is a no-op");
            }
        }
        Ok(())
    })();
    tracing::info!("WinOps: fullscreen_nonnative exit");
    result
}
