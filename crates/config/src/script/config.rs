use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex},
};

use ruau::vm::{Limits, LoadedModule, Vm};

use super::{ModeRef, util::lock_unpoisoned};
use crate::{Style, StyleProvenance};

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
    /// Retained ruau VM used for later renders and handler execution.
    pub(crate) vm: Vm,
    /// Loaded root module retained for the VM lifetime.
    pub(crate) _root_module: LoadedModule,
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

    /// Collect garbage left unreachable by completed config entrypoints.
    pub(crate) fn collect_entrypoint_garbage(&mut self) {
        self.vm.collect();
    }

    /// Return the per-entrypoint execution limits.
    pub(crate) fn entry_limits() -> Limits {
        Limits::production(SCRIPT_GAS_LIMIT, SCRIPT_MEMORY_LIMIT)
    }
}
