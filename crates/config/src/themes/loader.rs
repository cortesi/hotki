use std::{
    collections::HashMap,
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
};

use mlua::{Lua, LuaSerdeExt, StdLib};

use super::ThemeError;
use crate::{error::excerpt_at, raw, script};

/// Embedded built-in theme sources included at compile time.
const BUILTIN_THEME_SOURCES: &[(&str, &str)] = &[
    (
        "default",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../themes/default.luau"
        )),
    ),
    (
        "charcoal",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../themes/charcoal.luau"
        )),
    ),
    (
        "dark-blue",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../themes/dark-blue.luau"
        )),
    ),
    (
        "solarized-dark",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../themes/solarized-dark.luau"
        )),
    ),
    (
        "solarized-light",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../themes/solarized-light.luau"
        )),
    ),
];

/// Load and evaluate all built-in embedded themes into raw style overlays.
pub fn load_builtin_raw_themes() -> Result<HashMap<&'static str, raw::RawStyle>, ThemeError> {
    let mut themes = HashMap::new();
    for (name, source) in BUILTIN_THEME_SOURCES {
        let path = PathBuf::from(format!("<builtin:{name}>"));
        themes.insert(*name, eval_theme_source(source, &path)?);
    }

    if !themes.contains_key("default") {
        return Err(ThemeError::Validation {
            path: PathBuf::from("<builtin>"),
            line: None,
            col: None,
            message: "built-in theme registry must include 'default'".to_string(),
            excerpt: None,
        });
    }

    Ok(themes)
}

/// Load and evaluate `*.luau` theme files from the provided directory.
pub fn load_user_raw_themes(dir: &Path) -> Result<HashMap<String, raw::RawStyle>, ThemeError> {
    if !dir.exists() {
        return Ok(HashMap::new());
    }
    if !dir.is_dir() {
        return Err(ThemeError::Read {
            path: dir.to_path_buf(),
            message: "themes path exists but is not a directory".to_string(),
        });
    }

    let mut paths = fs::read_dir(dir)
        .map_err(|err| ThemeError::Read {
            path: dir.to_path_buf(),
            message: err.to_string(),
        })?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension() == Some(OsStr::new("luau")))
        .collect::<Vec<_>>();
    paths.sort();

    let mut themes = HashMap::new();
    for path in paths {
        let Some(stem) = path.file_stem().and_then(OsStr::to_str) else {
            return Err(ThemeError::Validation {
                path,
                line: None,
                col: None,
                message: "theme filename must be valid UTF-8".to_string(),
                excerpt: None,
            });
        };
        let name = stem.to_string();
        let raw = load_theme_file(&path)?;
        if themes.insert(name.clone(), raw).is_some() {
            return Err(ThemeError::Validation {
                path,
                line: None,
                col: None,
                message: format!("duplicate theme name from filename: {name}"),
                excerpt: None,
            });
        }
    }

    Ok(themes)
}

/// Read and evaluate one filesystem-backed theme file.
fn load_theme_file(path: &Path) -> Result<raw::RawStyle, ThemeError> {
    let source = fs::read_to_string(path).map_err(|err| ThemeError::Read {
        path: path.to_path_buf(),
        message: err.to_string(),
    })?;
    eval_theme_source(&source, path)
}

/// Evaluate one Luau theme source file into a raw style overlay.
fn eval_theme_source(source: &str, path: &Path) -> Result<raw::RawStyle, ThemeError> {
    let lua = Lua::new_with(StdLib::ALL_SAFE, mlua::LuaOptions::default()).map_err(|err| {
        ThemeError::Validation {
            path: path.to_path_buf(),
            line: None,
            col: None,
            message: err.to_string(),
            excerpt: None,
        }
    })?;
    lua.sandbox(true).map_err(|err| ThemeError::Validation {
        path: path.to_path_buf(),
        line: None,
        col: None,
        message: err.to_string(),
        excerpt: None,
    })?;

    let value = lua
        .load(source)
        .set_name(path.to_string_lossy().as_ref())
        .eval()
        .map_err(|err| error_from_luau(source, &err, path))?;

    lua.from_value(value).map_err(|err| ThemeError::Validation {
        path: path.to_path_buf(),
        line: None,
        col: None,
        message: format!("invalid theme table: {err}"),
        excerpt: None,
    })
}

/// Convert an `mlua` error into a source-located theme error.
fn error_from_luau(source: &str, err: &mlua::Error, path: &Path) -> ThemeError {
    let (line, col) = parse_error_location(err).unwrap_or((1, 1));
    ThemeError::Parse {
        path: path.to_path_buf(),
        line,
        col,
        message: err.to_string(),
        excerpt: excerpt_at(source, line, col),
    }
}

/// Extract a `(line, column)` pair from a Luau error message.
fn parse_error_location(err: &mlua::Error) -> Option<(usize, usize)> {
    let (_, line, col) = script::parse_error_location(err)?;
    Some((line.unwrap_or(1), col.unwrap_or(1)))
}
