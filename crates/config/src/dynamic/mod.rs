//! Dynamic configuration model backed by Rhai closures.

mod config;
mod dsl;
mod render;
mod types;

pub use config::DynamicConfig;
pub use render::{RenderOutput, render_stack, resolve_binding};
pub use types::{
    ActionCtx, Binding, BindingFlags, BindingKind, Effect, HandlerRef, HudRow, HudRowStyle, ModeCtx,
    ModeFrame, ModeId, ModeRef, NavRequest, RenderedState, RepeatSpec, StyleOverlay,
};
