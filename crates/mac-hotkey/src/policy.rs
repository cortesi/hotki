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
