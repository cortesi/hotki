//! Built-in selector helpers for installed macOS applications.

use std::{
    env,
    ffi::OsStr,
    fs, io,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use rhai::{Array, Dynamic, Engine, EvalAltResult, NativeCallContext, Position};

use super::{SelectorItem, dsl::DynamicConfigScriptState, util::lock_unpoisoned};

/// Register `get_applications()` in the Rhai DSL.
pub(super) fn register_apps_api(engine: &mut Engine, state: Arc<Mutex<DynamicConfigScriptState>>) {
    engine.register_fn(
        "get_applications",
        move |ctx: NativeCallContext| -> Result<Array, Box<EvalAltResult>> {
            let cached = { lock_unpoisoned(&state).applications_cache.clone() };
            if let Some(apps) = cached {
                return Ok(selector_items_to_array(apps.as_ref()));
            }

            let apps = scan_applications().map_err(|err| {
                boxed_runtime_error(format!("get_applications: {}", err), ctx.call_position())
            })?;
            let apps: Arc<[SelectorItem]> = apps.into();
            {
                lock_unpoisoned(&state).applications_cache = Some(apps.clone());
            }
            Ok(selector_items_to_array(apps.as_ref()))
        },
    );
}

/// Convert an IO error into a Rhai runtime error at the call site.
fn boxed_runtime_error(message: String, pos: Position) -> Box<EvalAltResult> {
    Box::new(EvalAltResult::ErrorRuntime(Dynamic::from(message), pos))
}

/// Scan installed applications from standard macOS directories.
fn scan_applications() -> io::Result<Vec<SelectorItem>> {
    scan_applications_in_dirs(&application_roots())
}

/// Standard directories used for application bundle discovery.
fn application_roots() -> Vec<PathBuf> {
    let mut roots = vec![
        PathBuf::from("/Applications"),
        PathBuf::from("/System/Applications"),
        PathBuf::from("/Applications/Utilities"),
    ];
    if let Some(home) = env::var_os("HOME").map(PathBuf::from) {
        roots.push(home.join("Applications"));
    }
    roots
}

/// Scan all roots and return a sorted selector item list.
fn scan_applications_in_dirs(roots: &[PathBuf]) -> io::Result<Vec<SelectorItem>> {
    let mut bundles = Vec::new();
    for root in roots {
        if !root.is_dir() {
            continue;
        }
        scan_applications_under(root, &mut bundles)?;
    }

    let mut items = bundles
        .iter()
        .filter_map(|path| selector_item_for_app(path))
        .collect::<Vec<_>>();
    items.sort_by(|a, b| {
        a.label
            .cmp(&b.label)
            .then_with(|| a.sublabel.cmp(&b.sublabel))
    });
    items.dedup_by(|a, b| a.sublabel == b.sublabel);
    Ok(items)
}

/// Recursively scan `root` for `.app` bundles without descending into bundles.
fn scan_applications_under(root: &Path, out: &mut Vec<PathBuf>) -> io::Result<()> {
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = fs::read_dir(&dir)?;
        for entry in entries {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };

            let ty = match entry.file_type() {
                Ok(t) => t,
                Err(_) => continue,
            };
            if ty.is_symlink() {
                continue;
            }

            let path = entry.path();
            if ty.is_dir() {
                if is_app_bundle(&path) {
                    out.push(path);
                } else {
                    stack.push(path);
                }
            }
        }
    }
    Ok(())
}

/// True when `path` is a directory with an `.app` extension.
fn is_app_bundle(path: &Path) -> bool {
    path.extension() == Some(OsStr::new("app"))
}

/// Convert an application bundle path into a selector item.
fn selector_item_for_app(path: &Path) -> Option<SelectorItem> {
    let label = path.file_stem()?.to_string_lossy().to_string();
    let full = path.to_string_lossy().to_string();
    Some(SelectorItem {
        label,
        sublabel: Some(full.clone()),
        data: Dynamic::from(full),
    })
}

/// Convert selector items to a Rhai array of item maps.
fn selector_items_to_array(items: &[SelectorItem]) -> Array {
    items
        .iter()
        .map(|item| {
            let mut map = rhai::Map::new();
            map.insert("label".into(), Dynamic::from(item.label.clone()));
            map.insert(
                "sublabel".into(),
                item.sublabel
                    .as_ref()
                    .map(|s| Dynamic::from(s.clone()))
                    .unwrap_or(Dynamic::UNIT),
            );
            map.insert("data".into(), item.data.clone());
            Dynamic::from(map)
        })
        .collect()
}
