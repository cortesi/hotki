use std::fmt::{Display, Formatter, Result as FmtResult};

use thiserror::Error;

use crate::{geom::Rect, place::AttemptTimeline};

/// Bitflags-style struct capturing which edges were clamped to the
/// visible frame during placement verification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ClampFlags {
    /// Window's left edge equals the visible frame's left edge (≈ within eps).
    pub left: bool,
    /// Window's right edge equals the visible frame's right edge (≈ within eps).
    pub right: bool,
    /// Window's top edge equals the visible frame's top edge (≈ within eps).
    pub top: bool,
    /// Window's bottom edge equals the visible frame's bottom edge (≈ within eps).
    pub bottom: bool,
}

impl ClampFlags {
    /// Returns true if any clamp flag is set.
    pub fn any(self) -> bool {
        self.left || self.right || self.top || self.bottom
    }
}

impl Display for ClampFlags {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        let mut parts: Vec<&str> = Vec::new();
        if self.left {
            parts.push("left");
        }
        if self.right {
            parts.push("right");
        }
        if self.bottom {
            parts.push("bottom");
        }
        if self.top {
            parts.push("top");
        }
        if parts.is_empty() {
            write!(f, "none")
        } else {
            write!(f, "{}", parts.join(","))
        }
    }
}

/// Errors that can occur during window operations.
#[derive(Debug)]
pub struct PlacementErrorDetails {
    pub expected: Rect,
    pub got: Rect,
    pub epsilon: f64,
    pub clamped: ClampFlags,
    pub visible_frame: Rect,
    pub timeline: AttemptTimeline,
}

impl Display for PlacementErrorDetails {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        write!(
            f,
            "expected={:?} got={:?} eps={:.2} clamped={} vf={:?} attempts={}",
            self.expected, self.got, self.epsilon, self.clamped, self.visible_frame, self.timeline
        )
    }
}

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

    /// The window is in macOS system Full Screen (separate Space) where
    /// AX-driven frame changes are unsupported. Caller should bail early.
    #[error("unsupported: fullscreen active")]
    FullscreenActive,

    /// An invalid index was provided.
    #[error("Invalid index")]
    InvalidIndex,

    /// Failed to activate the application.
    #[error("Activation failed")]
    ActivationFailed,

    /// Target rectangle appears at global (0,0) while operating relative to a
    /// non‑primary screen (non‑zero screen origin). This usually indicates the
    /// caller passed screen‑local coordinates instead of global coordinates.
    #[error("bad coord space: target (0,0) on non-primary screen")]
    BadCoordinateSpace,

    /// Post‑placement verification failed: the window's actual frame did not
    /// match the requested target within `epsilon` tolerance.
    #[error("post-placement verification failed in {op}: {details}")]
    PlacementVerificationFailed {
        /// Logical operation name (e.g., "place_grid").
        op: &'static str,
        /// Detailed failure diagnostics captured by the engine.
        details: Box<PlacementErrorDetails>,
    },
}

pub type Result<T> = std::result::Result<T, Error>;
