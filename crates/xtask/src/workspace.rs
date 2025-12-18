//! Workspace discovery and metadata helpers.

use std::{
    fs,
    path::{Path, PathBuf},
};

use crate::{Error, Result};

/// Determine the workspace root directory.
pub fn workspace_root() -> Result<PathBuf> {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .ancestors()
        .nth(2)
        .map(Path::to_path_buf)
        .ok_or(Error::WorkspaceRootNotFound)
}

/// Extract the workspace version from `[workspace.package]` in `Cargo.toml`.
pub fn workspace_version(root_dir: &Path) -> Result<String> {
    let manifest_path = root_dir.join("Cargo.toml");
    let manifest_bytes = fs::read(&manifest_path).map_err(|source| Error::Io {
        path: manifest_path.clone(),
        source,
    })?;
    let manifest = String::from_utf8(manifest_bytes).map_err(|source| Error::Utf8 {
        path: manifest_path.clone(),
        source,
    })?;

    let mut in_section = false;
    for raw_line in manifest.lines() {
        let line = raw_line.trim();
        if line.starts_with('[') {
            in_section = line == "[workspace.package]";
            continue;
        }
        if !in_section {
            continue;
        }
        if let Some(version) = parse_toml_string_kv(line, "version") {
            return Ok(version);
        }
    }

    Err(Error::MissingWorkspaceVersion {
        path: manifest_path,
    })
}

/// Parse a simple `key = "value"` TOML line.
fn parse_toml_string_kv(line: &str, key: &str) -> Option<String> {
    let line = line.split('#').next()?.trim();
    let (lhs, rhs) = line.split_once('=')?;
    if lhs.trim() != key {
        return None;
    }
    let rhs = rhs.trim();
    let rhs = rhs.strip_prefix('"')?.strip_suffix('"')?;
    Some(rhs.to_string())
}
