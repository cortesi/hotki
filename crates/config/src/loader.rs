//! Parse and load user configuration.

use std::{ffi::OsStr, path::Path};

use crate::{Config, Error, RhaiRuntime, rhai::load_from_path_with_runtime};

/// Fully loaded configuration for server execution.
pub struct LoadedConfig {
    /// Parsed and resolved configuration.
    pub config: Config,
    /// Optional Rhai runtime for executing script actions.
    pub rhai: Option<RhaiRuntime>,
}

/// Load a fully resolved `Config` from a Rhai file at `path`.
pub fn load_from_path(path: &Path) -> Result<Config, Error> {
    Ok(load_for_server_from_path(path)?.config)
}

/// Load a config from disk and include any Rhai runtime needed for script actions.
pub fn load_for_server_from_path(path: &Path) -> Result<LoadedConfig, Error> {
    if path.extension() != Some(OsStr::new("rhai")) {
        return Err(Error::Read {
            path: Some(path.to_path_buf()),
            message: "Unsupported config format (expected a .rhai file)".to_string(),
        });
    }

    let loaded = load_from_path_with_runtime(path)?;
    Ok(LoadedConfig {
        config: loaded.config,
        rhai: loaded.runtime,
    })
}
