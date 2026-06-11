//! Luau-backed configuration runtime.

/// Application discovery for selector providers.
mod apps;
/// Binding-level style parsing helpers.
mod binding_style;
/// Loaded config state and source tracking.
mod config;
pub(crate) mod diagnostics;
pub mod engine;
/// Handler execution bridge.
mod handler;
mod host_action;
mod host_args;
mod host_hotki;
mod host_parse;
mod host_runtime;
mod host_themes;
pub(crate) mod host_userdata;
/// Shared Luau import roles and filesystem policy.
pub(crate) mod imports;
mod loader;
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
