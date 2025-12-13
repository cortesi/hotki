//! Parse and load configuration from RON sources.

use std::{fs, path::Path};

use crate::{Config, ConfigInput, Error, themes};

/// Load a fully resolved `Config` from a RON file at `path`.
///
/// - Selects the base theme using the `base_theme` field in the file (or
///   falls back to the default theme when absent).
/// - Applies any style overrides on top of the chosen base theme.
/// - Uses the user's keys when provided; otherwise falls back to empty keys.
pub fn load_from_path(path: &Path) -> Result<Config, Error> {
    let s = fs::read_to_string(path).map_err(|e| Error::Read {
        path: Some(path.to_path_buf()),
        message: e.to_string(),
    })?;
    load_from_str(&s, Some(path))
}

/// Parse a RON config string into a resolved `Config`.
///
/// `path` is only used to enrich error messages.
pub fn load_from_str(s: &str, path: Option<&Path>) -> Result<Config, Error> {
    match ron::from_str::<ConfigInput>(s) {
        Ok(user_in) => {
            // Determine base theme from the input (defaults to "default")
            let theme_to_use = user_in.base_theme.as_deref();
            let style_base = themes::load_theme(theme_to_use);

            // Build config; user overlay stored for later application.
            let mut cfg = Config::from_parts(user_in.keys, style_base);
            cfg.user_overlay = user_in.style;
            if let Some(tunables) = user_in.server {
                cfg.server = tunables;
            }
            Ok(cfg)
        }
        Err(err) => Err(Error::from_ron(s, &err, path)),
    }
}
