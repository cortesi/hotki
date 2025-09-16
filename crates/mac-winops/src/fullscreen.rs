use mac_keycode::{Chord, Key, Modifier};
use objc2_foundation::MainThreadMarker;
use relaykey::RelayKey;
use tracing::debug;

pub(crate) use crate::focused_window_for_pid;
use crate::{
    Desired, Error, Result,
    ax::{ax_bool, ax_check, ax_get_point, ax_get_size, ax_set_bool, cfstr},
    error::PlacementErrorDetails,
    frame_storage::{PREV_FRAMES, PREV_FRAMES_CAP},
    frontmost_window_for_pid,
    geom::Rect,
    place::{
        PlaceAttemptOptions, PlacementContext, PlacementEngine, PlacementEngineConfig,
        PlacementGrid, PlacementOutcome, normalize_before_move, skip_reason_for_role_subrole,
    },
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
        let front_info = frontmost_window_for_pid(pid);
        let prev_key = front_info.as_ref().map(|w| (pid, w.id));
        let front_id = front_info.as_ref().map(|w| w.id);

        normalize_before_move(&win, pid, front_id)?;

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
        let cur_rect = Rect::from((cur_p, cur_s));
        let vf = visible_frame_containing_point(mtm, cur_p);
        debug!("WinOps: visible frame {}", vf);
        let target_full = Rect::new(vf.x, vf.y, vf.w, vf.h);
        let is_full = cur_rect.approx_eq(&target_full, 1.0);
        let do_set_to_full = match desired {
            Desired::On => true,
            Desired::Off => false,
            Desired::Toggle => !is_full,
        };

        if do_set_to_full {
            tracing::debug!(
                "WinOps: fullscreen_nonnative applying engine | pid={} cur={} target={}",
                pid,
                cur_rect,
                target_full
            );
            if let Some(k) = prev_key {
                let mut map = PREV_FRAMES.lock();
                if map.len() >= PREV_FRAMES_CAP
                    && let Some(old_k) = map.keys().next().cloned()
                {
                    let _ = map.remove(&old_k);
                }
                map.entry(k).or_insert((cur_p, cur_s));
            }
            let ctx =
                PlacementContext::new(win.clone(), target_full, vf, PlaceAttemptOptions::default());
            let tuning = ctx.tuning();
            let engine = PlacementEngine::new(
                &ctx,
                PlacementEngineConfig {
                    label: "fullscreen_nonnative",
                    attr_pos,
                    attr_size,
                    grid: PlacementGrid {
                        cols: 1,
                        rows: 1,
                        col: 0,
                        row: 0,
                    },
                    role: &role,
                    subrole: &subrole,
                },
            );
            match engine.execute(mtm)? {
                PlacementOutcome::Verified(success) => {
                    debug!(
                        "WinOps: fullscreen_nonnative verified | pid={} target={} got={}",
                        pid, target_full, success.final_rect
                    );
                    Ok(())
                }
                PlacementOutcome::PosFirstOnlyFailure(failure)
                | PlacementOutcome::VerificationFailure(failure) => {
                    Err(Error::PlacementVerificationFailed {
                        op: "fullscreen_nonnative",
                        details: Box::new(PlacementErrorDetails {
                            expected: target_full,
                            got: failure.got,
                            epsilon: tuning.epsilon(),
                            clamped: failure.clamped,
                            visible_frame: failure.visible_frame,
                            timeline: failure.timeline,
                        }),
                    })
                }
            }
        } else {
            tracing::debug!("WinOps: restoring from previous frame if any");
            let mut restored = false;
            if let Some(k) = prev_key {
                let mut map = PREV_FRAMES.lock();
                if let Some((p, s)) = map.remove(&k) {
                    restored = true;
                    let target_restore = Rect::from((p, s));
                    if !target_restore.approx_eq(&cur_rect, 1.0) {
                        let vf_restore =
                            visible_frame_containing_point(mtm, target_restore.center());
                        let ctx = PlacementContext::new(
                            win.clone(),
                            target_restore,
                            vf_restore,
                            PlaceAttemptOptions::default(),
                        );
                        let tuning = ctx.tuning();
                        let engine = PlacementEngine::new(
                            &ctx,
                            PlacementEngineConfig {
                                label: "fullscreen_restore",
                                attr_pos,
                                attr_size,
                                grid: PlacementGrid {
                                    cols: 1,
                                    rows: 1,
                                    col: 0,
                                    row: 0,
                                },
                                role: &role,
                                subrole: &subrole,
                            },
                        );
                        match engine.execute(mtm)? {
                            PlacementOutcome::Verified(success) => {
                                debug!(
                                    "WinOps: fullscreen_nonnative restore verified | pid={} target={} got={}",
                                    pid, target_restore, success.final_rect
                                );
                            }
                            PlacementOutcome::PosFirstOnlyFailure(failure)
                            | PlacementOutcome::VerificationFailure(failure) => {
                                return Err(Error::PlacementVerificationFailed {
                                    op: "fullscreen_restore",
                                    details: Box::new(PlacementErrorDetails {
                                        expected: target_restore,
                                        got: failure.got,
                                        epsilon: tuning.epsilon(),
                                        clamped: failure.clamped,
                                        visible_frame: failure.visible_frame,
                                        timeline: failure.timeline,
                                    }),
                                });
                            }
                        }
                    }
                }
            }
            if !restored {
                debug!("no previous frame to restore; non-native Off is a no-op");
            }
            Ok(())
        }
    })();
    tracing::info!("WinOps: fullscreen_nonnative exit");
    result
}
