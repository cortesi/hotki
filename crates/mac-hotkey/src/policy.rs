#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Decision {
    pub emit: bool,
    pub intercept: bool,
}

/// Classify how the tap should handle a given event.
///
/// - If suspended, a new press is neither emitted nor intercepted.
/// - If not matched, nothing is emitted or intercepted.
/// - If matched, emit the new key-down to the client. Interception is controlled
///   by the registration.
///
/// Repeat and key-up policy comes from the press record retained by the event-tap
/// classifier rather than being rematched here.
pub fn classify(suspended: bool, matched_intercept: Option<bool>) -> Decision {
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
        let d = classify(true, M);
        assert!(!d.emit);
        assert!(!d.intercept);
    }

    #[test]
    fn non_match_emits_nothing() {
        let d = classify(false, None);
        assert!(!d.emit);
        assert!(!d.intercept);
    }

    #[test]
    fn matched_emits_and_intercept_tracks_option() {
        let d = classify(false, M);
        assert!(d.emit);
        assert!(!d.intercept);
        let d = classify(false, MI);
        assert!(d.emit);
        assert!(d.intercept);
    }
}
