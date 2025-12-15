//! Dynamic configuration model backed by Rhai closures.

/// Dynamic config container and style helpers.
mod config;
/// Rhai DSL registration and builder types.
mod dsl;
/// Handler execution helpers.
mod handler;
/// Config loading and Rhai engine setup.
mod loader;
/// Render pipeline turning closures into bindings.
mod render;
/// Core dynamic configuration types.
mod types;
/// Small internal utilities shared across dynamic config modules.
mod util;

pub use config::DynamicConfig;
pub use handler::{HandlerResult, execute_handler};
pub use loader::load_dynamic_config;
#[cfg(test)]
pub(crate) use loader::load_dynamic_config_from_string;
pub use render::{RenderOutput, render_stack, resolve_binding};
pub use types::{
    ActionCtx, Binding, BindingFlags, BindingKind, Effect, HandlerRef, HudRow, HudRowStyle,
    ModeCtx, ModeFrame, ModeId, ModeRef, NavRequest, RenderedState, RepeatSpec, StyleOverlay,
};
