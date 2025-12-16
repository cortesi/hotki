use std::{
    collections::HashMap,
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
};

use rhai::{AST, Dynamic, Engine, EvalAltResult, Position, Scope, serde::from_dynamic};

use super::ThemeError;
use crate::{dynamic::constants::register_style_constants, error::excerpt_at, raw};

/// Embedded built-in theme sources included at compile time.
const BUILTIN_THEME_SOURCES: &[(&str, &str)] = &[
    (
        "default",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../themes/default.rhai"
        )),
    ),
    (
        "charcoal",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../themes/charcoal.rhai"
        )),
    ),
    (
        "dark-blue",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../themes/dark-blue.rhai"
        )),
    ),
    (
        "solarized-dark",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../themes/solarized-dark.rhai"
        )),
    ),
    (
        "solarized-light",
        include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../themes/solarized-light.rhai"
        )),
    ),
];

/// Load and evaluate all built-in embedded themes into raw style overlays.
pub fn load_builtin_raw_themes() -> Result<HashMap<&'static str, raw::RawStyle>, ThemeError> {
    let mut themes = HashMap::new();
    for (name, source) in BUILTIN_THEME_SOURCES {
        let path = PathBuf::from(format!("<builtin:{}>", name));
        let raw = eval_theme_source(source, &path)?;
        themes.insert(*name, raw);
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

/// Load and evaluate `*.rhai` theme files from the provided directory.
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
        .map_err(|e| ThemeError::Read {
            path: dir.to_path_buf(),
            message: e.to_string(),
        })?
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|p| p.extension() == Some(OsStr::new("rhai")))
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
                message: format!("duplicate theme name from filename: {}", name),
                excerpt: None,
            });
        }
    }

    Ok(themes)
}

/// Read and evaluate a single theme file.
fn load_theme_file(path: &Path) -> Result<raw::RawStyle, ThemeError> {
    let source = fs::read_to_string(path).map_err(|e| ThemeError::Read {
        path: path.to_path_buf(),
        message: e.to_string(),
    })?;
    eval_theme_source(&source, path)
}

/// Evaluate theme source into a raw style overlay.
fn eval_theme_source(source: &str, path: &Path) -> Result<raw::RawStyle, ThemeError> {
    let mut engine = Engine::new();
    configure_engine(&mut engine);
    register_style_constants(&mut engine);

    let ast = compile(&engine, source, path)?;
    let mut scope = Scope::new();
    let result = engine
        .eval_ast_with_scope::<Dynamic>(&mut scope, &ast)
        .map_err(|err| error_from_rhai(source, &err, path))?;

    if !result.is::<rhai::Map>() {
        return Err(ThemeError::Validation {
            path: path.to_path_buf(),
            line: None,
            col: None,
            message: "theme script must evaluate to a map".to_string(),
            excerpt: None,
        });
    }

    from_dynamic(&result).map_err(|e| ThemeError::Validation {
        path: path.to_path_buf(),
        line: None,
        col: None,
        message: format!("invalid theme map: {}", e),
        excerpt: None,
    })
}

/// Configure the restricted Rhai engine used for theme evaluation.
fn configure_engine(engine: &mut Engine) {
    engine.on_print(|_| {});
    engine.on_debug(|_, _, _| {});

    engine.set_max_operations(50_000);
    engine.set_max_call_levels(32);
    engine.set_max_expr_depths(96, 48);
}

/// Compile theme source code into an AST.
fn compile(engine: &Engine, source: &str, path: &Path) -> Result<AST, ThemeError> {
    engine
        .compile(source)
        .map_err(|err| error_from_rhai(source, &err.into(), path))
}

/// Convert a Rhai error into a source-located theme parse error with an excerpt.
fn error_from_rhai(source: &str, err: &EvalAltResult, path: &Path) -> ThemeError {
    let (line, col) = pos_to_line_col(err.position()).unwrap_or((1, 1));
    ThemeError::Parse {
        path: path.to_path_buf(),
        line,
        col,
        message: err.to_string(),
        excerpt: excerpt_at(source, line, col),
    }
}

/// Convert a Rhai position into a 1-based (line, column) pair.
fn pos_to_line_col(pos: Position) -> Option<(usize, usize)> {
    let line = pos.line()?;
    let col = pos.position().unwrap_or(1);
    Some((line.max(1), col.max(1)))
}
