use std::{path::PathBuf, sync::Arc};

use rhai::{AST, Engine};

use super::ModeRef;
use crate::{Style, raw, themes};

/// A loaded dynamic configuration consisting of a root mode closure plus style and Rhai runtime.
pub struct DynamicConfig {
    /// Root mode closure registered via `hotki.mode(...)`.
    pub(crate) root: ModeRef,
    /// Optional base theme name selected via `base_theme("...")`.
    pub(crate) base_theme: Option<String>,
    /// Optional user style overlay selected via `style(#{...})`.
    pub(crate) user_style: Option<raw::RawStyle>,
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

    /// Base theme name selected by the config (if set).
    pub fn base_theme(&self) -> Option<&str> {
        self.base_theme.as_deref()
    }

    /// Compute base style for the config, including optional theme and user-style overrides.
    pub fn base_style(&self, theme_override: Option<&str>, user_style_enabled: bool) -> Style {
        let theme = theme_override.or(self.base_theme.as_deref());
        let mut style = themes::load_theme(theme);
        if user_style_enabled && let Some(overlay) = self.user_style.as_ref() {
            style = style.overlay_raw(overlay);
        }
        style
    }
}
