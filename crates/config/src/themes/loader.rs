use std::{
    collections::HashMap,
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
};

use oxau::{
    compile::{self, CompileError, CompileOptions},
    embed::{Scope, ScopedValue, ScriptError, serde::from_scoped_value},
    profile::Profile,
    session::{Ambient, Limits, Vm},
};

use super::ThemeError;
use crate::{error::excerpt_at, raw};

/// Gas budget for evaluating a single theme source.
const THEME_GAS_LIMIT: u64 = 1_000_000;

/// Heap budget for the short-lived VM used to evaluate one theme.
const THEME_MEMORY_LIMIT: usize = 16 * 1024 * 1024;

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
    let profile = Profile::full();
    let chunk = compile::compile_for(
        &profile,
        source.as_bytes(),
        &CompileOptions::for_vm_execution(),
    )
    .map_err(|err| compile_error_to_theme(source, &err, path))?;
    let chunk_name = chunk_name(path);
    let mut vm = build_theme_vm(profile, path)?;
    let module = vm
        .load_named(&chunk, chunk_name.as_bytes())
        .map_err(|err| validation_error(path, err.to_string()))?;

    let mut parsed = None;
    let mut script_error = None;
    let mut decode_error = None;
    vm.step_with_limits(theme_limits(), |scope| {
        let main = scope.module_function(&module);
        let result: Result<ScopedValue<'_>, ScriptError<'_>> = scope.call_protected(main, ())?;
        match result {
            Ok(value) => match from_scoped_value::<raw::RawStyle>(scope, value) {
                Ok(style) => parsed = Some(style),
                Err(err) => decode_error = Some(err.message().to_string()),
            },
            Err(err) => script_error = Some(script_error_to_theme(source, path, scope, &err)),
        }
        Ok(())
    })
    .map_err(|err| validation_error(path, err.message()))?;

    if let Some(err) = script_error {
        return Err(err);
    }
    if let Some(message) = decode_error {
        return Err(ThemeError::Validation {
            path: path.to_path_buf(),
            line: None,
            col: None,
            message: format!("invalid theme table: {message}"),
            excerpt: None,
        });
    }

    parsed.ok_or_else(|| validation_error(path, "theme script returned no value"))
}

/// Build the sandboxed VM used to evaluate one theme.
fn build_theme_vm(profile: Profile, path: &Path) -> Result<Vm, ThemeError> {
    Vm::builder()
        .ambient(Ambient::deterministic(0))
        .limits(theme_limits())
        .profile(profile)
        .build_sandboxed()
        .map_err(|err| validation_error(path, err.to_string()))
}

/// Return the per-theme execution limits.
fn theme_limits() -> Limits {
    Limits::production(THEME_GAS_LIMIT, THEME_MEMORY_LIMIT)
}

/// Convert a filesystem or built-in theme path into an oxau chunk name.
fn chunk_name(path: &Path) -> String {
    format!("@{}", path.display())
}

/// Build a locationless theme validation error.
fn validation_error(path: &Path, message: impl Into<String>) -> ThemeError {
    ThemeError::Validation {
        path: path.to_path_buf(),
        line: None,
        col: None,
        message: message.into(),
        excerpt: None,
    }
}

/// Convert a structured oxau compile error into a theme error.
fn compile_error_to_theme(source: &str, err: &CompileError, path: &Path) -> ThemeError {
    let Some(location) = err.location() else {
        return validation_error(path, err.message());
    };

    let line = location.begin.line as usize + 1;
    let col = location.begin.column as usize + 1;
    ThemeError::Parse {
        path: path.to_path_buf(),
        line,
        col,
        message: err.message().to_string(),
        excerpt: excerpt_at(source, line, col),
    }
}

/// Convert a protected script failure into a located validation error when possible.
fn script_error_to_theme<'s>(
    source: &str,
    path: &Path,
    scope: &Scope<'s>,
    err: &ScriptError<'s>,
) -> ThemeError {
    let message = script_error_message(scope, err);
    let (line, col, excerpt) = traceback_location(err.traceback(), path)
        .map(|(line, col)| (Some(line), Some(col), Some(excerpt_at(source, line, col))))
        .unwrap_or((None, None, None));
    ThemeError::Validation {
        path: path.to_path_buf(),
        line,
        col,
        message,
        excerpt,
    }
}

/// Extract a readable message from a scoped script error value.
fn script_error_message<'s>(scope: &Scope<'s>, err: &ScriptError<'s>) -> String {
    from_scoped_value::<String>(scope, err.value())
        .unwrap_or_else(|_| format!("theme script raised a {} value", err.value().type_name()))
}

/// Extract the first frame for this theme path from oxau's traceback text.
fn traceback_location(traceback: Option<&str>, path: &Path) -> Option<(usize, usize)> {
    let expected = path.to_string_lossy();
    traceback?.lines().find_map(|line| {
        let suffix = line.trim().strip_prefix(expected.as_ref())?;
        let digits = suffix
            .strip_prefix(':')?
            .chars()
            .take_while(char::is_ascii_digit)
            .collect::<String>();
        let line = digits.parse::<usize>().ok()?;
        Some((line, 1))
    })
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    #[test]
    fn valid_theme_loads_through_oxau() {
        let style = eval_theme_source(
            r##"
            return {
                hud = {
                    font_size = 15,
                    title_fg = "#ffffff",
                },
            }
            "##,
            Path::new("<test-theme>"),
        )
        .expect("theme loads");

        assert_eq!(
            style.hud.as_option().unwrap().font_size.as_option(),
            Some(&15.0)
        );
    }

    #[test]
    fn parse_error_reports_structured_location() {
        let err = eval_theme_source("return {\n  hud = @\n}", Path::new("<test-theme>"))
            .expect_err("theme must fail to parse");

        let ThemeError::Parse {
            line, col, excerpt, ..
        } = err
        else {
            panic!("expected parse error");
        };
        assert!(line > 0);
        assert!(col > 0);
        assert!(excerpt.contains('^'));
    }

    #[test]
    fn runtime_error_reports_traceback_location() {
        let err = eval_theme_source(
            r#"
            error("boom")
            "#,
            Path::new("<test-theme>"),
        )
        .expect_err("theme must fail at runtime");

        let ThemeError::Validation {
            line,
            col,
            message,
            excerpt,
            ..
        } = err
        else {
            panic!("expected validation error");
        };
        assert_eq!(line, Some(2));
        assert_eq!(col, Some(1));
        assert!(message.contains("boom"));
        assert!(excerpt.unwrap().contains('^'));
    }
}
