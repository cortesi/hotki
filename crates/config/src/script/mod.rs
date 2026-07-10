//! Luau-backed configuration runtime.

/// Application discovery for selector providers.
mod apps;
/// Retained callback ownership and call-context plumbing.
mod callback;
/// Loaded config state and source tracking.
mod config;
pub(crate) mod diagnostics;
pub mod engine;
/// Handler execution bridge.
mod handler;
mod host_args;
mod host_hotki;
mod host_parse;
mod host_runtime;
pub(crate) mod host_userdata;
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
#[cfg(test)]
pub(crate) use loader::load_dynamic_config_from_string;
#[cfg(test)]
pub(crate) use render::render_stack;
#[cfg(test)]
pub(crate) use selector::SelectorItems;
pub(crate) use selector::{SelectorConfig, SelectorData, SelectorItem};
pub(crate) use types::{
    ActionCtx, ActionRepeatPermission, Binding, BindingFlags, BindingKind, Effect, HandlerRef,
    ModeCtx, ModeFrame, ModeRef, NavRequest, RenderedState, RepeatSpec, SourcePos,
};
