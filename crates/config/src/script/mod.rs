//! Luau-backed configuration runtime.

/// Application discovery for selector providers.
mod apps;
/// Binding-level style parsing helpers.
mod binding_style;
/// Loaded config state and source tracking.
mod config;
pub mod engine;
/// Handler execution bridge.
mod handler;
/// Luau loader and host API installation.
mod loader;
/// Mode-stack rendering and error mapping.
mod render;
/// Selector parsing and runtime types.
mod selector;
/// Shared runtime data types.
mod types;
/// Small synchronization and locking helpers.
mod util;

#[cfg(test)]
mod test_script;

pub(crate) use config::DynamicConfig;
pub(crate) use loader::load_dynamic_config_from_string;
pub(crate) use render::parse_error_location;
#[cfg(test)]
pub(crate) use render::render_stack;
pub(crate) use selector::{SelectorConfig, SelectorItem, SelectorItems};
pub(crate) use types::{
    ActionCtx, Binding, BindingFlags, BindingKind, BindingStyle, Effect, HandlerRef, ModeCtx,
    ModeFrame, ModeRef, NavRequest, RenderedState, RepeatSpec, SourcePos, StyleOverlay,
};
