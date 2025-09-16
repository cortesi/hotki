use tracing::debug;

use super::common::{POLL_SLEEP_MS, POLL_TOTAL_MS, sleep_ms};
use crate::{
    Error, Result, WindowId,
    ax::{ax_bool, ax_perform_action, ax_set_bool, cfstr},
};

#[inline]
pub(super) fn skip_reason_for_role_subrole(role: &str, subrole: &str) -> Option<&'static str> {
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

/// Best‑effort window state normalization prior to placement:
/// - Bail if system Full Screen is active.
/// - If minimized/zoomed, turn off and wait briefly.
/// - Try to raise the window (ignore unsupported/failed).
pub(super) fn normalize_before_move(
    win: &crate::AXElem,
    pid: i32,
    id_opt: Option<WindowId>,
) -> Result<()> {
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

    // Track if we changed window state that can affect AX settable bits.
    let mut changed_state = false;

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
            changed_state = true;
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
            changed_state = true;
        }
        Ok(Some(false)) => {}
        _ => {}
    }

    // If we toggled minimized/zoomed, clear cached settable flags so subsequent
    // placement re-queries AXIsAttributeSettable with the updated window state.
    if changed_state {
        crate::ax::ax_clear_settable_cache(win.as_ptr());
    }

    // 4) Best‑effort raise: prefer our AX window; for known id, also use raise helper.
    // Try direct AXRaise on the window first.
    let _ = ax_perform_action(win.as_ptr(), cfstr("AXRaise"));
    if let Some(id) = id_opt {
        let _ = crate::raise::raise_window(pid, id);
    }
    Ok(())
}
// Pre‑placement normalization and role/subrole skip logic.

#[cfg(test)]
mod tests {
    use super::skip_reason_for_role_subrole;

    #[test]
    fn skips_sheet_by_role() {
        assert_eq!(
            skip_reason_for_role_subrole("AXSheet", "AXStandardWindow"),
            Some("role=AXSheet")
        );
    }

    #[test]
    fn skips_popover_when_marked_in_role_or_subrole() {
        assert_eq!(
            skip_reason_for_role_subrole("AXPopover", "AXStandardWindow"),
            Some("popover")
        );
        assert_eq!(
            skip_reason_for_role_subrole("AXWindow", "AXPopover"),
            Some("popover")
        );
    }

    #[test]
    fn skips_dialog_variants() {
        assert_eq!(
            skip_reason_for_role_subrole("AXWindow", "AXDialog"),
            Some("dialog")
        );
        assert_eq!(
            skip_reason_for_role_subrole("AXWindow", "AXSystemDialog"),
            Some("dialog")
        );
    }

    #[test]
    fn skips_floating_palettes() {
        assert_eq!(
            skip_reason_for_role_subrole("AXWindow", "AXFloatingWindow"),
            Some("floating")
        );
    }

    #[test]
    fn allows_standard_windows() {
        assert_eq!(
            skip_reason_for_role_subrole("AXWindow", "AXStandardWindow"),
            None
        );
    }
}
