//! Luau-backed configuration runtime.

/// Application discovery for selector providers.
mod apps;
/// Retained callback ownership and call-context plumbing.
mod callback;
/// Loaded config state and source tracking.
pub mod config;
pub mod diagnostics;
/// Handler execution bridge.
pub mod handler;
mod host_args;
mod host_hotki;
mod host_parse;
mod host_runtime;
pub mod host_userdata;
pub mod loader;
/// Checked and cached filesystem module source.
mod module_source;
pub mod render;
/// Selector parsing and runtime types.
pub mod selector;
/// Shared runtime data types.
pub mod types;
/// Small synchronization and locking helpers.
mod util;

#[cfg(test)]
mod test_script;

pub use config::LoadedConfig;
#[cfg(test)]
pub use loader::load_dynamic_config_from_string;
#[cfg(test)]
pub use render::render_stack;
#[cfg(test)]
pub use selector::SelectorItems;
pub use selector::{SelectorConfig, SelectorData, SelectorItem};
pub use types::{
    ActionCtx, ActionRepeatPermission, Binding, BindingFlags, BindingKind, Effect, HandlerRef,
    ModeCtx, ModeFrame, ModeRef, NavRequest, RenderedState, RepeatSpec, SourcePos,
};
