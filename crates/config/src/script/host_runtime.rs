//! Shared state and utility types for Luau host modules.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use ruau::embed::StashedClosure;

use super::{
    HandlerRef, ModeRef, SelectorItem, config::SourceMap, imports::ImportRole,
    util::lock_unpoisoned,
};
use crate::raw;

/// Shared mutable state captured by native host functions installed into one VM.
pub(super) type SharedRuntimeState = Arc<Mutex<RuntimeState>>;

/// Mutable loader state shared across the Luau runtime.
#[derive(Debug, Clone, Default)]
pub(super) struct RuntimeState {
    /// Root mode declared by `hotki.root(...)`.
    pub(super) root: Option<ModeRef>,
    /// Theme registry after built-in, user, and script registration.
    pub(super) themes: HashMap<String, raw::RawStyle>,
    /// Active theme selected during loading.
    pub(super) active_theme: String,
    /// Cached application selector items.
    pub(super) applications_cache: Option<Arc<[SelectorItem]>>,
    /// Directory containing the root config file.
    pub(super) config_dir: Option<PathBuf>,
    /// Source text cache used for excerpts and diagnostics.
    pub(super) sources: SourceMap,
    /// Imported role modules keyed by `(role, canonical_path)`.
    pub(super) imports: HashMap<(ImportRole, PathBuf), ImportedValue>,
}

/// Cached imported values stored in loader state.
#[derive(Clone, Debug)]
pub(super) enum ImportedValue {
    /// Imported mode renderer.
    Mode(ModeRef),
    /// Imported selector item provider or static list.
    Items(ImportedItems),
    /// Imported action handler.
    Handler(HandlerRef),
    /// Imported style overlay.
    Style(Box<raw::RawStyle>),
}

/// Imported selector item values.
#[derive(Clone, Debug)]
pub(super) enum ImportedItems {
    /// Imported item provider closure.
    Provider(StashedClosure),
    /// Imported static selector item list.
    Static(Vec<SelectorItem>),
}

/// Render a display name for an optional source path.
pub(super) fn chunk_name(path: Option<&Path>) -> String {
    path.map(|path| format!("@{}", path.display()))
        .unwrap_or_else(|| "=<memory>".to_string())
}

/// Read the active source map from shared runtime state.
pub(super) fn clone_sources(state: &SharedRuntimeState) -> SourceMap {
    lock_unpoisoned(state).sources.clone()
}
