//! Dynamic configuration model backed by Rhai closures.

mod config;
mod dsl;
mod handler;
mod render;
mod types;

pub use config::DynamicConfig;
pub use handler::{HandlerResult, execute_handler};
pub use render::{RenderOutput, render_stack, resolve_binding};
pub use types::{
    ActionCtx, Binding, BindingFlags, BindingKind, Effect, HandlerRef, HudRow, HudRowStyle, ModeCtx,
    ModeFrame, ModeId, ModeRef, NavRequest, RenderedState, RepeatSpec, StyleOverlay,
};
