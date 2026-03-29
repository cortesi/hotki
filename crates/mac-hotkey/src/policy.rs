use crate::EventKind;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Decision {
    pub emit: bool,
    pub intercept: bool,
}

/// Classify how the tap should handle a given event.
///
/// - If suspended, nothing is emitted or intercepted.
/// - If not matched, nothing is emitted or intercepted.
/// - If matched, always emit to the client (including OS auto-repeat KeyDown).
///   Interception is controlled by registration regardless of repeat.
pub fn classify(
    suspended: bool,
    matched_intercept: Option<bool>,
    _kind: EventKind,
    is_repeat: bool,
) -> Decision {
    if suspended {
        return Decision {
            emit: false,
            intercept: false,
        };
    }
    let Some(intercept) = matched_intercept else {
        return Decision {
            emit: false,
            intercept: false,
        };
    };
    // Emit to client for both initial presses and repeats; interception unchanged.
    let _ = is_repeat; // repeat does not affect emission policy anymore
    Decision {
        emit: true,
        intercept,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const M: Option<bool> = Some(false);
    const MI: Option<bool> = Some(true);

    #[test]
    fn suspended_ignores_everything() {
        let d = classify(true, M, EventKind::KeyDown, false);
        assert!(!d.emit);
        assert!(!d.intercept);
    }

    #[test]
    fn non_match_emits_nothing() {
        let d = classify(false, None, EventKind::KeyDown, false);
        assert!(!d.emit);
        assert!(!d.intercept);
    }

    #[test]
    fn match_repeat_is_emitted_and_intercept_tracks_option() {
        let d = classify(false, M, EventKind::KeyDown, true);
        assert!(d.emit);
        assert!(!d.intercept);
        let d = classify(false, MI, EventKind::KeyDown, true);
        assert!(d.emit);
        assert!(d.intercept);
    }

    #[test]
    fn match_initial_emits_and_intercept_tracks_option() {
        let d = classify(false, M, EventKind::KeyDown, false);
        assert!(d.emit);
        assert!(!d.intercept);
        let d = classify(false, MI, EventKind::KeyUp, false);
        assert!(d.emit);
        assert!(d.intercept);
    }
}
