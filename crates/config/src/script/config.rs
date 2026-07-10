use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex},
};

use ruau::{
    host::RetainedRuntime,
    vm::{CallOptions, Limits},
};

use super::{
    ModeRef,
    callback::{CallbackContext, CallbackRegistry, SharedCallbackRegistry},
    diagnostics,
    util::lock_unpoisoned,
};
use crate::{Error, Style, StyleProvenance};

/// Gas budget for each dynamic config entrypoint.
pub const SCRIPT_GAS_LIMIT: u64 = 4_000_000;

/// Heap budget for the retained dynamic config VM.
pub const SCRIPT_MEMORY_LIMIT: usize = 32 * 1024 * 1024;

/// Shared loaded sources used for error excerpts.
pub type SourceMap = Arc<Mutex<HashMap<PathBuf, Arc<str>>>>;

/// A loaded Luau configuration consisting of a root mode plus the runtime.
pub struct DynamicConfig {
    /// Root mode renderer declared by `hotki.root(...)`.
    pub(crate) root: ModeRef,
    /// Resolved base style loaded from the embedded default and optional sibling override.
    pub(crate) base_style: Style,
    /// Source of the resolved base style.
    pub(crate) style_provenance: StyleProvenance,
    /// Retained Ruau runtime used for later renders and handler execution.
    pub(crate) runtime: RetainedRuntime,
    /// Callback promotion and deferred-release registry.
    callbacks: SharedCallbackRegistry,
    /// Optional origin path for the loaded config.
    pub(crate) path: Option<PathBuf>,
    /// Cached source text for excerpts and diagnostics.
    pub(crate) sources: SourceMap,
}

impl DynamicConfig {
    /// Root mode closure for this config.
    pub fn root(&self) -> ModeRef {
        self.root.clone()
    }

    /// Return the resolved base style.
    pub fn base_style(&self) -> Style {
        self.base_style.clone()
    }

    /// Return the source of the resolved base style.
    pub fn style_provenance(&self) -> &StyleProvenance {
        &self.style_provenance
    }

    /// Return the resolved style and its provenance.
    pub fn resolved_style(&self) -> crate::ResolvedStyle {
        crate::ResolvedStyle {
            style: self.base_style.clone(),
            provenance: self.style_provenance.clone(),
        }
    }

    /// Return cached source text for a known filesystem path.
    pub(crate) fn source_for(&self, path: &PathBuf) -> Option<Arc<str>> {
        lock_unpoisoned(&self.sources).get(path).cloned()
    }

    /// Return the per-entrypoint execution limits.
    pub(crate) fn entry_limits() -> Limits {
        Limits::production(SCRIPT_GAS_LIMIT, SCRIPT_MEMORY_LIMIT)
    }

    /// Build complete options for one config entrypoint.
    pub(crate) fn entry_options() -> CallOptions {
        CallOptions::new().limits(Self::entry_limits())
    }

    /// Build the borrowed callback context for one config entrypoint.
    pub(crate) fn callback_context(&self) -> CallbackContext {
        CallbackContext::new(Arc::clone(&self.callbacks))
    }

    /// Promote newly stashed callbacks and release callbacks whose last owner dropped.
    pub(crate) fn synchronize_callbacks(&mut self) -> Result<(), Error> {
        CallbackRegistry::synchronize(&self.callbacks, &mut self.runtime)
            .map_err(|error| diagnostics::config_retained_error(self.path.clone(), &error))
    }

    /// Assemble a loaded config from its retained runtime and root callback.
    pub(super) fn new(
        root: ModeRef,
        base_style: Style,
        style_provenance: StyleProvenance,
        runtime: RetainedRuntime,
        callbacks: SharedCallbackRegistry,
        path: Option<PathBuf>,
        sources: SourceMap,
    ) -> Self {
        Self {
            root,
            base_style,
            style_provenance,
            runtime,
            callbacks,
            path,
            sources,
        }
    }

    /// Allocate the callback registry used while loading a config.
    pub(super) fn callback_registry() -> SharedCallbackRegistry {
        Arc::new(Mutex::new(CallbackRegistry::default()))
    }
}

impl Drop for DynamicConfig {
    fn drop(&mut self) {
        drop(CallbackRegistry::synchronize(
            &self.callbacks,
            &mut self.runtime,
        ));
        self.runtime.invalidate();
    }
}
