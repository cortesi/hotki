//! Shared state and utility types for Luau host modules.

use std::{
    path::Path,
    sync::{Arc, Mutex},
};

use super::{ModeRef, SelectorItem};
/// Shared mutable state captured by native host functions installed into one VM.
pub(super) type SharedRuntimeState = Arc<Mutex<RuntimeState>>;

/// Mutable loader state shared across the Luau runtime.
#[derive(Debug, Clone, Default)]
pub(super) struct RuntimeState {
    /// Root mode declared by `hotki.root(...)`.
    pub(super) root: Option<ModeRef>,
    /// Cached application selector items.
    pub(super) applications_cache: Option<Arc<[SelectorItem]>>,
}

/// Render a display name for an optional source path.
pub(super) fn chunk_name(path: Option<&Path>) -> String {
    path.map(|path| format!("@{}", path.display()))
        .unwrap_or_else(|| "=<memory>".to_string())
}
