use std::{error::Error as StdError, fmt};

use rhai::{Dynamic, EvalAltResult, Position};

#[derive(Debug, Clone)]
/// Validation error used to surface user-facing diagnostics with a source location.
pub struct ValidationError {
    /// Error message to surface.
    pub(crate) message: String,
}

impl fmt::Display for ValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl StdError for ValidationError {}

/// Create a boxed Rhai runtime error tagged as a validation error.
pub(super) fn boxed_validation_error(message: String, pos: Position) -> Box<EvalAltResult> {
    Box::new(EvalAltResult::ErrorRuntime(
        Dynamic::from(ValidationError { message }),
        pos,
    ))
}

/// Extract a structured validation error previously emitted by the DSL.
pub(super) fn extract_validation_error(err: &EvalAltResult) -> Option<(Position, String)> {
    match err {
        EvalAltResult::ErrorRuntime(d, pos) if d.is::<ValidationError>() => {
            let ve: ValidationError = d.clone_cast();
            Some((*pos, ve.message))
        }
        EvalAltResult::ErrorInFunctionCall(_, _, inner, _)
        | EvalAltResult::ErrorInModule(_, inner, _) => extract_validation_error(inner),
        _ => None,
    }
}
