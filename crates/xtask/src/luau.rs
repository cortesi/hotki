//! Luau validation tasks.

use std::{fs, path::Path};

use config::{check_luau_config, check_luau_theme_dir, luau_api, themes};

use crate::{Error, Result};

/// Validate the checked-in Luau API, built-in themes, and example configs.
pub fn luau(root_dir: &Path) -> Result<()> {
    println!("==> checked-in Luau API");
    let _ = luau_api();

    println!("==> built-in themes");
    themes::init_builtins();
    let repo_theme_dir = root_dir.join("themes");
    let theme_count = check_luau_theme_dir(&repo_theme_dir).map_err(|source| Error::Luau {
        path: repo_theme_dir.clone(),
        message: source.pretty(),
    })?;
    println!("validated {theme_count} theme files");

    println!("==> example configs");
    let example_dir = root_dir.join("examples");
    let mut example_paths = fs::read_dir(&example_dir)
        .map_err(|source| Error::Io {
            path: example_dir.clone(),
            source,
        })?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().is_some_and(|ext| ext == "luau"))
        .collect::<Vec<_>>();
    example_paths.sort();

    for path in &example_paths {
        let report = check_luau_config(path).map_err(|source| Error::Luau {
            path: path.clone(),
            message: source.pretty(),
        })?;
        println!(
            "{}: {} imports, {} theme files",
            path.strip_prefix(root_dir)
                .unwrap_or(path)
                .to_string_lossy(),
            report.imports,
            report.themes
        );
    }

    Ok(())
}
