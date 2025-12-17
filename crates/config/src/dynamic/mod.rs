//! Dynamic configuration model backed by Rhai closures.

/// Built-in application scanning helpers for selectors.
mod apps;
/// Dynamic config container and style helpers.
mod config;
/// Global constant registration shared by the DSL and theme loader.
pub(crate) mod constants;
/// Rhai DSL registration and builder types.
mod dsl;
/// Handler execution helpers.
mod handler;
/// Config loading and Rhai engine setup.
mod loader;
/// Render pipeline turning closures into bindings.
mod render;
/// Selector binding configuration types.
mod selector;
/// Style and theme APIs exposed to Rhai.
mod style_api;
/// Core dynamic configuration types.
mod types;
/// Small internal utilities shared across dynamic config modules.
mod util;
/// Shared validation error helpers for user-facing diagnostics.
mod validation;

#[cfg(test)]
mod test_dynamic;

pub use config::DynamicConfig;
pub use handler::{HandlerResult, execute_handler, execute_selector_handler};
pub use loader::load_dynamic_config;
#[cfg(test)]
pub(crate) use loader::load_dynamic_config_from_string;
pub use render::{RenderOutput, render_stack, resolve_binding};
pub use selector::{SelectorConfig, SelectorItem, SelectorItems};
pub use types::{
    ActionCtx, Binding, BindingFlags, BindingKind, Effect, HandlerRef, HudRow, HudRowStyle,
    ModeCtx, ModeFrame, ModeId, ModeRef, NavRequest, RenderedState, RepeatSpec, StyleOverlay,
};
