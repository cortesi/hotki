#![allow(clippy::module_name_repetitions)]

use tracing::Level;

use crate::ax::K_AX_ERROR_CANNOT_COMPLETE;

/// Domains where the window-ops layer tracks known status codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StatusKind {
    /// Accessibility observer installation (AXObserverAddNotification etc.).
    AxObserverAttach,
    /// CoreGraphics window ordering (CGSOrderWindow, private SkyLight APIs).
    CgsOrderWindow,
}

/// Logging policy for a status code that we intentionally demote.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StatusPolicy {
    /// Tracing level to emit for the status.
    pub level: Level,
    /// Short note describing why the status is considered expected noise.
    pub note: &'static str,
}

const K_CG_ERROR_FAILURE: i32 = 1000;

/// Return the policy for a known noisy status code, if any.
pub fn policy(kind: StatusKind, code: i32) -> Option<StatusPolicy> {
    match kind {
        StatusKind::AxObserverAttach => match code {
            K_AX_ERROR_CANNOT_COMPLETE => Some(StatusPolicy {
                level: Level::DEBUG,
                note: "Accessibility server timed out during observer registration; retry later.",
            }),
            _ => None,
        },
        StatusKind::CgsOrderWindow => match code {
            K_CG_ERROR_FAILURE => Some(StatusPolicy {
                level: Level::TRACE,
                note: "kCGErrorFailure when reordering a window owned by another process; fall back to activation.",
            }),
            _ => None,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ax_observer_policy_matches_expected_level() {
        let policy = policy(StatusKind::AxObserverAttach, K_AX_ERROR_CANNOT_COMPLETE)
            .expect("policy exists for AX cannot-complete");
        assert_eq!(policy.level, Level::DEBUG);
    }

    #[test]
    fn cgs_order_window_failure_is_demoted_to_trace() {
        let policy = policy(StatusKind::CgsOrderWindow, K_CG_ERROR_FAILURE)
            .expect("policy exists for CGS failure");
        assert_eq!(policy.level, Level::TRACE);
    }
}
