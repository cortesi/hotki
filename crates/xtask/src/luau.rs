//! Luau validation tasks.

use std::{fs, path::Path};

use config::{
    LuauApiSurface, check_luau_config, check_luau_style_file, check_luau_style_source,
    default_style_source, luau_api_surface,
};

use crate::{Error, Result};

/// Validate the checked-in Luau API, embedded style, and example configs.
pub fn luau(root_dir: &Path) -> Result<()> {
    println!("==> checked-in Luau API");
    let _ = luau_api_surface(LuauApiSurface::All);

    println!("==> embedded default style");
    let default_style_path = root_dir.join("crates/config/styles/default.luau");
    check_luau_style_source(&default_style_path, default_style_source()).map_err(|source| {
        Error::Luau {
            path: default_style_path.clone(),
            message: source.pretty(),
        }
    })?;

    println!("==> example style");
    let example_style_path = root_dir.join("examples/style.luau");
    let style_present =
        check_luau_style_file(&example_style_path).map_err(|source| Error::Luau {
            path: example_style_path.clone(),
            message: source.pretty(),
        })?;
    println!("validated example style: {style_present}");

    println!("==> example configs");
    let example_dir = root_dir.join("examples");
    let mut example_paths = fs::read_dir(&example_dir)
        .map_err(|source| Error::Io {
            path: example_dir.clone(),
            source,
        })?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| {
            path.extension().is_some_and(|ext| ext == "luau")
                && path.file_name().is_some_and(|name| name != "style.luau")
        })
        .collect::<Vec<_>>();
    example_paths.sort();

    for path in &example_paths {
        let report = check_luau_config(path).map_err(|source| Error::Luau {
            path: path.clone(),
            message: source.pretty(),
        })?;
        println!(
            "{}: style file: {}",
            path.strip_prefix(root_dir)
                .unwrap_or(path)
                .to_string_lossy(),
            report.style
        );
    }

    println!("==> screenshot config");
    let screenshot_config_path = root_dir.join("crates/hotki-shots/fixtures/config.luau");
    let report = check_luau_config(&screenshot_config_path).map_err(|source| Error::Luau {
        path: screenshot_config_path.clone(),
        message: source.pretty(),
    })?;
    println!(
        "{}: style file: {}",
        screenshot_config_path
            .strip_prefix(root_dir)
            .unwrap_or(&screenshot_config_path)
            .to_string_lossy(),
        report.style
    );

    Ok(())
}
