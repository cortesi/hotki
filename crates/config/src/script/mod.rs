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
/// Shared Luau import roles and filesystem policy.
pub(crate) mod imports;
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
#[cfg(test)]
pub(crate) use render::render_stack;
#[cfg(test)]
pub(crate) use selector::SelectorItems;
pub(crate) use selector::{SelectorConfig, SelectorData, SelectorItem};
pub(crate) use types::{
    ActionCtx, Binding, BindingFlags, BindingKind, BindingStyle, Effect, HandlerRef, ModeCtx,
    ModeFrame, ModeRef, NavRequest, RenderedState, RepeatSpec, SourcePos, StyleOverlay,
};
