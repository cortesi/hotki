//! Luau validation tasks.

use std::{
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
};

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
    let mut example_paths = example_config_paths(&example_dir)?;
    example_paths.sort();

    for path in &example_paths {
        let report = check_luau_config(path).map_err(|source| Error::Luau {
            path: path.clone(),
            message: source.pretty(),
        })?;
        println!(
            "{}: modules: {}, style file: {}",
            path.strip_prefix(root_dir)
                .unwrap_or(path)
                .to_string_lossy(),
            report.modules,
            report.style
        );
    }

    validate_markdown_configs(root_dir)?;

    println!("==> screenshot config");
    let screenshot_config_path = root_dir.join("crates/hotki-shots/fixtures/config.luau");
    let report = check_luau_config(&screenshot_config_path).map_err(|source| Error::Luau {
        path: screenshot_config_path.clone(),
        message: source.pretty(),
    })?;
    println!(
        "{}: modules: {}, style file: {}",
        screenshot_config_path
            .strip_prefix(root_dir)
            .unwrap_or(&screenshot_config_path)
            .to_string_lossy(),
        report.modules,
        report.style
    );

    Ok(())
}

/// Find top-level example files and nested directories whose entry is `config.luau`.
fn example_config_paths(root: &Path) -> Result<Vec<PathBuf>> {
    fn visit(root: &Path, directory: &Path, paths: &mut Vec<PathBuf>) -> Result<()> {
        let entries = fs::read_dir(directory).map_err(|source| Error::Io {
            path: directory.to_path_buf(),
            source,
        })?;
        for entry in entries {
            let entry = entry.map_err(|source| Error::Io {
                path: directory.to_path_buf(),
                source,
            })?;
            let path = entry.path();
            if path.is_dir() {
                visit(root, &path, paths)?;
                continue;
            }
            let is_top_level = path.parent() == Some(root);
            let is_top_level_example = is_top_level
                && path.extension() == Some(OsStr::new("luau"))
                && path.file_name() != Some(OsStr::new("style.luau"));
            let is_nested_entry =
                !is_top_level && path.file_name() == Some(OsStr::new("config.luau"));
            if is_top_level_example || is_nested_entry {
                paths.push(path);
            }
        }
        Ok(())
    }

    let mut paths = Vec::new();
    visit(root, root, &mut paths)?;
    Ok(paths)
}

/// Extract and validate documentation fences explicitly marked as complete configs.
fn validate_markdown_configs(root: &Path) -> Result<()> {
    println!("==> documentation configs");
    let output_dir = root.join("tmp/luau-docs");
    if output_dir.exists() {
        fs::remove_dir_all(&output_dir).map_err(|source| Error::Io {
            path: output_dir.clone(),
            source,
        })?;
    }
    fs::create_dir_all(&output_dir).map_err(|source| Error::Io {
        path: output_dir.clone(),
        source,
    })?;

    let mut count = 0;
    for name in ["README.md", "CONFIG.md"] {
        let document = root.join(name);
        let source = fs::read_to_string(&document).map_err(|source| Error::Io {
            path: document.clone(),
            source,
        })?;
        for (index, config) in marked_luau_configs(&document, &source)? {
            count += 1;
            let extracted = output_dir.join(format!(
                "{}-{index}.luau",
                name.trim_end_matches(".md").to_ascii_lowercase()
            ));
            fs::write(&extracted, config).map_err(|source| Error::Io {
                path: extracted.clone(),
                source,
            })?;
            let report = check_luau_config(&extracted).map_err(|source| Error::Luau {
                path: document.clone(),
                message: source.pretty(),
            })?;
            println!("{name} config {index}: modules: {}", report.modules);
        }
    }
    if count == 0 {
        return Err(Error::Luau {
            path: root.to_path_buf(),
            message: "no <!-- hotki-luau: config --> fences found".to_string(),
        });
    }
    Ok(())
}

/// Return `(fence index, source)` for complete config fences in one document.
fn marked_luau_configs(path: &Path, source: &str) -> Result<Vec<(usize, String)>> {
    const PREFIX: &str = "<!-- hotki-luau: ";
    let lines = source.lines().collect::<Vec<_>>();
    let mut configs = Vec::new();
    let mut cursor = 0;
    let mut fence_index = 0;
    while cursor < lines.len() {
        let line = lines[cursor].trim();
        let Some(kind) = line
            .strip_prefix(PREFIX)
            .and_then(|line| line.strip_suffix(" -->"))
        else {
            cursor += 1;
            continue;
        };
        if !matches!(kind, "config" | "fragment" | "module") {
            return Err(Error::Luau {
                path: path.to_path_buf(),
                message: format!("unknown hotki-luau fence kind '{kind}'"),
            });
        }
        cursor += 1;
        if lines.get(cursor).map(|line| line.trim()) != Some("```luau") {
            return Err(Error::Luau {
                path: path.to_path_buf(),
                message: format!("hotki-luau marker on line {cursor} must precede a Luau fence"),
            });
        }
        cursor += 1;
        let body_start = cursor;
        while cursor < lines.len() && lines[cursor].trim() != "```" {
            cursor += 1;
        }
        if cursor == lines.len() {
            return Err(Error::Luau {
                path: path.to_path_buf(),
                message: "unterminated marked Luau fence".to_string(),
            });
        }
        fence_index += 1;
        if kind == "config" {
            configs.push((
                fence_index,
                format!("{}\n", lines[body_start..cursor].join("\n")),
            ));
        }
        cursor += 1;
    }
    Ok(configs)
}
