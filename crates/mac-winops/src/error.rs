use thiserror::Error;

use crate::geom::Rect;

/// Errors that can occur during window operations.
#[derive(Error, Debug)]
pub enum Error {
    /// Accessibility permission is required but not granted.
    #[error("Accessibility permission missing")]
    Permission,

    /// Failed to create an Accessibility API application element.
    #[error("Failed to create AX application element")]
    AppElement,

    /// No focused window could be found for the given process.
    #[error("Focused window not available")]
    FocusedWindow,

    /// An Accessibility API operation failed with the given error code.
    #[error("AX operation failed: code {0}")]
    AxCode(i32),

    /// The AX element became invalid (e.g., window closed) during the operation.
    #[error("AX element invalid (window gone)")]
    WindowGone,

    /// Operation must be executed on the main thread.
    #[error("Operation requires main thread")]
    MainThread,

    /// The requested attribute or operation is not supported.
    #[error("Unsupported attribute")]
    Unsupported,

    /// An invalid index was provided.
    #[error("Invalid index")]
    InvalidIndex,

    /// Failed to activate the application.
    #[error("Activation failed")]
    ActivationFailed,

    /// Post‑placement verification failed: the window's actual frame did not
    /// match the requested target within `epsilon` tolerance.
    #[error(
        "post-placement verification failed in {op}: expected={expected:?} got={got:?} \
         eps={epsilon:.2} diff=(dx={dx:.2}, dy={dy:.2}, dw={dw:.2}, dh={dh:.2})"
    )]
    PlacementVerificationFailed {
        /// Logical operation name (e.g., "place_grid").
        op: &'static str,
        /// The requested target rectangle.
        expected: Rect,
        /// The actual rectangle observed after placement.
        got: Rect,
        /// Allowed absolute tolerance for each component.
        epsilon: f64,
        /// Absolute delta in x between expected and actual.
        dx: f64,
        /// Absolute delta in y between expected and actual.
        dy: f64,
        /// Absolute delta in width between expected and actual.
        dw: f64,
        /// Absolute delta in height between expected and actual.
        dh: f64,
    },
}

pub type Result<T> = std::result::Result<T, Error>;
