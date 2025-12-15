use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};

use rhai::{AST, Engine};

use crate::{Style, raw};

use super::ModeRef;

/// A loaded dynamic configuration consisting of a root mode closure plus style and Rhai runtime.
pub struct DynamicConfig {
    pub(crate) root: ModeRef,
    pub(crate) base_theme: Option<String>,
    pub(crate) user_style: Option<raw::RawStyle>,
    pub(crate) engine: Engine,
    pub(crate) ast: AST,
    pub(crate) source: Arc<str>,
    pub(crate) path: Option<PathBuf>,
    pub(crate) render_warnings: Arc<Mutex<Vec<String>>>,
}

impl DynamicConfig {
    pub(crate) fn base_style(&self, theme_override: Option<&str>, user_style_enabled: bool) -> Style {
        let theme = theme_override.or(self.base_theme.as_deref());
        let mut style = crate::themes::load_theme(theme);
        if user_style_enabled
            && let Some(overlay) = self.user_style.as_ref()
        {
            style = style.overlay_raw(overlay);
        }
        style
    }
}
