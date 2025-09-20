//! Frame reconciliation utilities for `hotki-world`.
//!
//! Authoritative frame selection obeys the following rules:
//! - **Normal/Hidden** windows prefer CoreGraphics geometry, falling back to
//!   Accessibility only when CG data is absent.
//! - **Fullscreen/Tiled** windows rely on CoreGraphics frames to avoid
//!   split-view discrepancies; Accessibility coordinates are used as a last
//!   resort when CG is unavailable.
//! - **Minimized** windows reuse the last visible rectangle observed before the
//!   minimize transition. The cached rectangle is marked with
//!   [`FrameKind::Cached`].
//!
//! Raw AX/CG rectangles are exposed exclusively under the
//! `test-introspection` feature flag to keep the default API surface lean.
use std::fmt;

use mac_winops::{self, Pos, Rect};

/// Pixel-space rectangle with integer coordinates.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RectPx {
    /// Horizontal origin (points) in pixels.
    pub x: i32,
    /// Vertical origin (points) in pixels.
    pub y: i32,
    /// Width in pixels.
    pub w: i32,
    /// Height in pixels.
    pub h: i32,
}

impl RectPx {
    /// A `0x0` rectangle at the origin.
    #[must_use]
    pub const fn zero() -> Self {
        Self {
            x: 0,
            y: 0,
            w: 0,
            h: 0,
        }
    }

    /// Construct from a CoreGraphics position record.
    #[must_use]
    pub fn from_pos(pos: &Pos) -> Self {
        Self {
            x: pos.x,
            y: pos.y,
            w: pos.width,
            h: pos.height,
        }
    }

    /// Construct from an Accessibility rectangle expressed in floats.
    #[must_use]
    pub fn from_ax(rect: &Rect) -> Self {
        Self {
            x: rect.x.round() as i32,
            y: rect.y.round() as i32,
            w: rect.w.round() as i32,
            h: rect.h.round() as i32,
        }
    }

    /// Compute the delta `(other - self)` in pixels.
    #[must_use]
    pub fn delta(&self, other: &Self) -> RectDelta {
        RectDelta {
            dx: other.x - self.x,
            dy: other.y - self.y,
            dw: other.w - self.w,
            dh: other.h - self.h,
        }
    }
}

/// Delta between two rectangles (`other - self`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RectDelta {
    /// Change in the horizontal origin.
    pub dx: i32,
    /// Change in the vertical origin.
    pub dy: i32,
    /// Change in width.
    pub dw: i32,
    /// Change in height.
    pub dh: i32,
}

impl fmt::Display for RectDelta {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "dx={} dy={} dw={} dh={}",
            self.dx, self.dy, self.dw, self.dh
        )
    }
}

/// Kind of frame being compared for diagnostics.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameKind {
    /// Frame sourced from the Accessibility (AX) API.
    Ax,
    /// Frame sourced from CoreGraphics (CG) data.
    Cg,
    /// Frame replayed from the cache (last unminimized).
    Cached,
    /// Frame synthesized due to missing data.
    Unknown,
}

/// Modes a window can be reconciled under.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowMode {
    /// Window is visible and operating normally.
    Normal,
    /// Window is minimized to the Dock.
    Minimized,
    /// Window is hidden or resident on an inactive space.
    Hidden,
    /// Window occupies a fullscreen space.
    Fullscreen,
    /// Window participates in macOS tile/split view.
    Tiled,
}

impl WindowMode {
    /// True when the window presents content on-screen.
    #[must_use]
    pub const fn is_visible(self) -> bool {
        matches!(self, Self::Normal | Self::Fullscreen | Self::Tiled)
    }
}

/// Aggregated frame metadata for a window.
#[derive(Clone, Debug, PartialEq)]
pub struct Frames {
    /// Currently authoritative frame in pixel coordinates.
    pub authoritative: RectPx,
    /// Source that produced the authoritative frame.
    pub authoritative_kind: FrameKind,
    #[cfg(feature = "test-introspection")]
    /// Raw AX frame (if available).
    pub ax: Option<RectPx>,
    #[cfg(feature = "test-introspection")]
    /// Raw CG frame (if available).
    pub cg: Option<RectPx>,
    /// Identifier of the display containing the window.
    pub display_id: Option<u32>,
    /// Mission Control space identifier.
    pub space_id: Option<i64>,
    /// Backing scale factor for the display.
    pub scale: f32,
    /// Derived window mode.
    pub mode: WindowMode,
}

impl Frames {
    /// Construct an empty placeholder entry.
    #[must_use]
    pub fn zero() -> Self {
        Self {
            authoritative: RectPx::zero(),
            authoritative_kind: FrameKind::Unknown,
            #[cfg(feature = "test-introspection")]
            ax: None,
            #[cfg(feature = "test-introspection")]
            cg: None,
            display_id: None,
            space_id: None,
            scale: 1.0,
            mode: WindowMode::Normal,
        }
    }
}

/// Default epsilon in integer pixels for a given backing scale.
#[must_use]
pub fn default_eps(scale: f32) -> i32 {
    if scale >= 1.5 { 1 } else { 0 }
}

/// Choose an authoritative frame from optional AX/CG rectangles and cached state.
#[must_use]
pub fn reconcile_authoritative(
    ax: Option<RectPx>,
    cg: Option<RectPx>,
    mode: WindowMode,
    last_unminimized: Option<RectPx>,
) -> (RectPx, FrameKind) {
    match mode {
        WindowMode::Minimized => {
            if let Some(rect) = last_unminimized {
                (rect, FrameKind::Cached)
            } else if let Some(rect) = cg {
                (rect, FrameKind::Cg)
            } else if let Some(rect) = ax {
                (rect, FrameKind::Ax)
            } else {
                (RectPx::zero(), FrameKind::Unknown)
            }
        }
        WindowMode::Fullscreen | WindowMode::Tiled => {
            if let Some(rect) = cg {
                (rect, FrameKind::Cg)
            } else if let Some(rect) = ax {
                (rect, FrameKind::Ax)
            } else {
                (RectPx::zero(), FrameKind::Unknown)
            }
        }
        _ => {
            if let Some(rect) = cg {
                (rect, FrameKind::Cg)
            } else if let Some(rect) = ax {
                (rect, FrameKind::Ax)
            } else {
                (RectPx::zero(), FrameKind::Unknown)
            }
        }
    }
}
