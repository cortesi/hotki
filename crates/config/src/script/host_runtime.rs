//! Application caching and source-name utilities for Luau host modules.

use std::{
    path::Path,
    sync::{Arc, Mutex},
};

use super::SelectorItem;
/// Application cache shared by native host functions installed into one VM.
pub(super) type SharedApplicationCache = Arc<Mutex<ApplicationCache>>;

/// Cached application selector items for one loaded configuration.
#[derive(Debug, Clone, Default)]
pub(super) struct ApplicationCache {
    /// Items discovered on the first `hotki.applications` call.
    pub(super) items: Option<Arc<[SelectorItem]>>,
}

/// Render a display name for an optional source path.
pub(super) fn chunk_name(path: Option<&Path>) -> String {
    path.map(|path| format!("@{}", path.display()))
        .unwrap_or_else(|| "=<memory>".to_string())
}
