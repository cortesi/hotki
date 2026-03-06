//! Build script that derives the workspace crate target list for logging filters.

use std::{
    env, fs,
    path::{Path, PathBuf},
};

fn main() {
    let workspace_root = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap())
        .parent()
        .and_then(Path::parent)
        .unwrap()
        .to_path_buf();
    let crates_dir = workspace_root.join("crates");

    println!(
        "cargo:rerun-if-changed={}",
        workspace_root.join("Cargo.toml").display()
    );
    println!("cargo:rerun-if-changed={}", crates_dir.display());

    let mut crate_names = workspace_crate_names(&crates_dir);
    crate_names.sort();

    let out_path = PathBuf::from(env::var("OUT_DIR").unwrap()).join("workspace_crates.rs");
    let body = format!(
        "/// Workspace crate targets included in default logging directives.\nconst OUR_CRATES: &[&str] = &[\n{}\n];\n",
        crate_names
            .iter()
            .map(|name| format!("    {:?},", name))
            .collect::<Vec<_>>()
            .join("\n")
    );
    fs::write(out_path, body).unwrap();
}

/// Read crate target names from workspace member manifests under `crates/`.
fn workspace_crate_names(crates_dir: &Path) -> Vec<String> {
    let mut crate_names = Vec::new();
    let entries = match fs::read_dir(crates_dir) {
        Ok(entries) => entries,
        Err(_) => return crate_names,
    };

    for entry in entries.filter_map(Result::ok) {
        let cargo_toml = entry.path().join("Cargo.toml");
        if !cargo_toml.is_file() {
            continue;
        }
        if let Some(name) = package_name(&cargo_toml) {
            crate_names.push(name.replace('-', "_"));
        }
    }

    crate_names
}

/// Extract the package name from a `Cargo.toml` `[package]` section.
fn package_name(cargo_toml: &Path) -> Option<String> {
    let contents = fs::read_to_string(cargo_toml).ok()?;
    let mut in_package = false;

    for line in contents.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_package = trimmed == "[package]";
            continue;
        }
        if !in_package {
            continue;
        }
        let Some(value) = trimmed.strip_prefix("name = ") else {
            continue;
        };
        return Some(value.trim_matches('"').to_string());
    }

    None
}
