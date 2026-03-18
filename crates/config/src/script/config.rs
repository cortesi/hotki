use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

use mlua::Lua;

use super::{ModeRef, util::lock_unpoisoned};
use crate::{Style, raw, style};

/// Shared loaded sources used for error excerpts.
pub type SourceMap = Arc<Mutex<HashMap<PathBuf, Arc<str>>>>;

/// A loaded Luau configuration consisting of a root mode plus the runtime.
pub struct DynamicConfig {
    /// Root mode renderer declared by `hotki.root(...)`.
    pub(crate) root: ModeRef,
    /// Theme registry after built-in, user, and script overrides are applied.
    pub(crate) themes: HashMap<String, raw::RawStyle>,
    /// Active theme selected while loading the config.
    pub(crate) active_theme: String,
    /// Backing Lua state used for later renders and handler execution.
    pub(crate) lua: Lua,
    /// Optional origin path for the loaded config.
    pub(crate) path: Option<PathBuf>,
    /// Cached source text for excerpts and diagnostics.
    pub(crate) sources: SourceMap,
    /// Shared execution-step counter backing the Luau interrupt hook.
    pub(crate) interrupt_steps: Arc<AtomicU64>,
}

impl DynamicConfig {
    /// Root mode closure for this config.
    pub fn root(&self) -> ModeRef {
        self.root.clone()
    }

    /// Return all registered theme names, sorted alphabetically.
    pub fn theme_names(&self) -> Vec<String> {
        let mut names = self.themes.keys().cloned().collect::<Vec<_>>();
        names.sort();
        names
    }

    /// Return true when a theme exists in this config's registry.
    pub fn theme_exists(&self, name: &str) -> bool {
        self.themes.contains_key(name)
    }

    /// Return the active theme name selected by the config.
    pub fn active_theme(&self) -> &str {
        self.active_theme.as_str()
    }

    /// Compute base style for the config, including optional theme override.
    pub fn base_style(&self, theme_override: Option<&str>) -> Style {
        let name = theme_override
            .filter(|n| !n.is_empty())
            .unwrap_or(self.active_theme());
        let Some(raw) = self.themes.get(name).or_else(|| self.themes.get("default")) else {
            return Style::default();
        };
        style::overlay_raw(Style::default(), raw)
    }

    /// Return cached source text for a known filesystem path.
    pub(crate) fn source_for(&self, path: &PathBuf) -> Option<Arc<str>> {
        lock_unpoisoned(&self.sources).get(path).cloned()
    }

    /// Reset the Luau execution budget before entering a fresh runtime callback.
    pub(crate) fn reset_execution_budget(&self) {
        self.interrupt_steps.store(0, Ordering::Relaxed);
    }
}
