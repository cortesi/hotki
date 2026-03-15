use std::{
    fs, io,
    path::{Path, PathBuf},
};

use mlua::{Lua, LuaSerdeExt, Result as LuaResult};
use serde::Serialize;

use super::SelectorItem;

/// Standard CoreServices applications directory scanned for built-in macOS apps.
const CORE_SERVICES_APPLICATIONS: &str = "/System/Library/CoreServices/Applications";
/// Finder bundle path, which sits outside the normal application roots.
const FINDER_APP_BUNDLE: &str = "/System/Library/CoreServices/Finder.app";

#[derive(Debug, Clone, Serialize)]
/// Serializable application metadata exposed to Luau selectors.
struct ApplicationInfo {
    /// User-visible application name.
    name: String,
    /// Absolute bundle path.
    path: String,
    /// Optional bundle identifier when available.
    bundle_id: Option<String>,
}

/// Build selector items for installed applications.
pub fn application_items(lua: &Lua) -> LuaResult<Vec<SelectorItem>> {
    scan_applications()
        .map(|apps| {
            apps.into_iter()
                .map(|app| {
                    let label = app.name.clone();
                    let sublabel = Some(app.path.clone());
                    let data = lua.to_value(&app)?;
                    Ok(SelectorItem {
                        label,
                        sublabel,
                        data,
                    })
                })
                .collect()
        })
        .map_err(mlua::Error::external)?
}

/// Scan known application roots and return deduplicated application metadata.
fn scan_applications() -> io::Result<Vec<ApplicationInfo>> {
    let roots = application_roots();
    let mut bundles = Vec::new();
    for root in roots {
        if root.exists() {
            scan_applications_under(&root, &mut bundles)?;
        }
    }

    bundles.sort();
    bundles.dedup();

    Ok(bundles
        .iter()
        .filter_map(|path| application_info(path))
        .collect())
}

/// Return the filesystem roots searched for installed application bundles.
fn application_roots() -> Vec<PathBuf> {
    vec![
        PathBuf::from("/Applications"),
        PathBuf::from("/System/Applications"),
        PathBuf::from(CORE_SERVICES_APPLICATIONS),
        PathBuf::from(FINDER_APP_BUNDLE),
    ]
}

/// Recursively discover `.app` bundles under `root`.
fn scan_applications_under(root: &Path, out: &mut Vec<PathBuf>) -> io::Result<()> {
    if is_app_bundle(root) {
        out.push(root.to_path_buf());
        return Ok(());
    }

    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            if is_app_bundle(&path) {
                out.push(path);
            } else {
                scan_applications_under(&path, out)?;
            }
        }
    }

    Ok(())
}

/// Return true when `path` is an application bundle directory.
fn is_app_bundle(path: &Path) -> bool {
    path.is_dir() && path.extension().is_some_and(|ext| ext == "app")
}

/// Build exported metadata for one application bundle.
fn application_info(path: &Path) -> Option<ApplicationInfo> {
    let name = path.file_stem()?.to_string_lossy().to_string();
    Some(ApplicationInfo {
        name,
        path: path.to_string_lossy().to_string(),
        bundle_id: None,
    })
}
