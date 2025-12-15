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
    pub(crate) fn base_style_theme(&self) -> Style {
        crate::themes::load_theme(self.base_theme.as_deref())
    }
}
