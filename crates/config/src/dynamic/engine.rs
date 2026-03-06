//! Engine-facing bridge types for the dynamic configuration runtime.
//!
//! This module is the intended cross-crate surface for `hotki-engine`.
//! The underlying render and frame machinery remains implemented in the
//! private `render` and `types` modules.

pub use super::{
    config::DynamicConfig,
    handler::{HandlerResult, execute_handler, execute_selector_handler},
    loader::load_dynamic_config,
    render::{RenderOutput, render_stack, resolve_binding},
    selector::{SelectorConfig, SelectorItem, SelectorItems},
    types::{
        ActionCtx, Binding, BindingFlags, BindingKind, Effect, HandlerRef, ModeCtx, ModeFrame,
        ModeId, ModeRef, NavRequest, RenderedState, RepeatSpec, StyleOverlay,
    },
};
