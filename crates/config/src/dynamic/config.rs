use std::{collections::HashMap, path::PathBuf, sync::Arc};

use rhai::{AST, Engine};

use super::ModeRef;
use crate::{Style, raw};

/// A loaded dynamic configuration consisting of a root mode closure plus style and Rhai runtime.
pub struct DynamicConfig {
    /// Root mode closure registered via `hotki.mode(...)`.
    pub(crate) root: ModeRef,
    /// Theme registry captured from the config script, including builtins.
    pub(crate) themes: HashMap<String, raw::RawStyle>,
    /// Active theme name selected by the config script via `theme("...")`.
    pub(crate) active_theme: String,
    /// Rhai engine used to execute mode closures and handlers.
    pub(crate) engine: Engine,
    /// Compiled Rhai AST for the loaded config.
    pub(crate) ast: AST,
    /// Full source text of the loaded config.
    pub(crate) source: Arc<str>,
    /// Optional path the config was loaded from.
    pub(crate) path: Option<PathBuf>,
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

        Style::default().overlay_raw(raw)
    }
}
