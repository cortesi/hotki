//! Engine-facing bridge types for the dynamic configuration runtime.
//!
//! This module is the intended cross-crate surface for `hotki-engine`.
//! The underlying render and frame machinery remains implemented in the
//! private `render` and `types` modules.

pub use super::{
    render::{RenderOutput, render_stack, resolve_binding},
    types::{
        ActionCtx, Binding, BindingFlags, BindingKind, Effect, HandlerRef, HudRow, HudRowStyle,
        ModeCtx, ModeFrame, ModeId, ModeRef, NavRequest, RenderedState, RepeatSpec, StyleOverlay,
    },
};
