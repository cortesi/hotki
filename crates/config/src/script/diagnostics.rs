//! Shared diagnostic conversion for Luau config, render, check, and theme paths.

use std::{
    fmt,
    path::{Path, PathBuf},
};

use ruau::{
    compile::CompileError,
    diagnostic::{DiagnosticLocation, TypeDiagnostic},
    embed::{RuntimeError, Scope, ScriptError, serde::from_scoped_value},
    session::{ProtectedScriptError, TracebackFrame},
};

use super::{config::SourceMap, util::lock_unpoisoned};
use crate::{Error, error::excerpt_at, themes::ThemeError};

/// Build a locationless `Error::Validation` with only a path and message.
pub fn config_validation(path: Option<PathBuf>, err: impl fmt::Display) -> Error {
    Error::Validation {
        path,
        line: None,
        col: None,
        message: err.to_string(),
        excerpt: None,
    }
}

/// Convert a structured ruau compile error into a config error.
pub fn config_compile_error(source: &str, err: &CompileError, path: Option<&Path>) -> Error {
    let Some((line, col)) = compile_location(err) else {
        return config_validation(path.map(Path::to_path_buf), err.message());
    };

    Error::Parse {
        path: path.map(Path::to_path_buf),
        line,
        col,
        message: err.message().to_string(),
        excerpt: excerpt_at(source, line, col),
    }
}

/// Convert an ruau runtime-surface error into a locationless config error.
pub fn config_runtime_error(path: Option<PathBuf>, err: &RuntimeError) -> Error {
    config_validation(path, err.message())
}

/// Convert a VM-level protected script failure into a located config error.
pub fn config_protected_error(
    source: &str,
    default_path: Option<&Path>,
    sources: &SourceMap,
    err: &ProtectedScriptError,
) -> Error {
    if let Some(error) = err.payload_ref::<Error>() {
        return error.clone();
    }

    let message = protected_error_message(err);
    let (path, line, col) = err
        .frames()
        .iter()
        .find_map(traceback_frame_location)
        .map(|(path, line)| {
            (
                normalize_chunk_path(path, default_path.map(Path::to_path_buf)),
                Some(line),
                Some(1),
            )
        })
        .unwrap_or((default_path.map(Path::to_path_buf), None, None));
    let excerpt = line.and_then(|line| {
        config_source_excerpt(Some(source), sources, path.as_ref(), line, col.unwrap_or(1))
    });

    Error::Validation {
        path,
        line,
        col,
        message,
        excerpt,
    }
}

/// Convert a protected script failure into a config error with a best-effort location.
pub fn config_script_error<'s>(
    default_path: Option<&Path>,
    sources: &SourceMap,
    scope: &Scope<'s>,
    err: &ScriptError<'s>,
) -> Error {
    if let Some(error) = err.payload_ref::<Error>() {
        return error.clone();
    }

    let message = script_error_message(scope, err, "script");
    let (path, line, col) = first_traceback_location(err.traceback())
        .map(|(path, line)| {
            (
                normalize_chunk_path(path, default_path.map(Path::to_path_buf)),
                Some(line),
                Some(1),
            )
        })
        .unwrap_or((default_path.map(Path::to_path_buf), None, None));
    let excerpt = line.and_then(|line| {
        config_source_excerpt(None, sources, path.as_ref(), line, col.unwrap_or(1))
    });

    Error::Validation {
        path,
        line,
        col,
        message,
        excerpt,
    }
}

/// Attach a source location and excerpt to a validation error at `offset`.
pub fn config_error_at_offset(path: &Path, source: &str, offset: usize, message: String) -> Error {
    let (line, col) = line_col_at(source, offset);
    Error::Validation {
        path: Some(path.to_path_buf()),
        line: Some(line),
        col: Some(col),
        message,
        excerpt: Some(excerpt_at(source, line, col)),
    }
}

/// Raise an already-shaped config error through Luau without losing its structure.
pub fn config_error_payload(error: Error) -> RuntimeError {
    RuntimeError::runtime(error.to_string()).with_payload(error)
}

/// Convert structured checker diagnostics into the stable config error shape.
pub fn config_type_error(
    path: &Path,
    source: &str,
    diagnostics: &[TypeDiagnostic],
    line_offset: usize,
) -> Error {
    let (line, col, excerpt) = diagnostics
        .first()
        .and_then(|diagnostic| source_position(diagnostic.primary_location, line_offset))
        .map(|(line, col)| (Some(line), Some(col), Some(excerpt_at(source, line, col))))
        .unwrap_or((None, None, None));
    Error::Validation {
        path: Some(path.to_path_buf()),
        line,
        col,
        message: render_type_diagnostics(path, diagnostics, line_offset),
        excerpt,
    }
}

/// Convert a structured ruau compile error into a theme error.
pub fn theme_compile_error(source: &str, err: &CompileError, path: &Path) -> ThemeError {
    let Some((line, col)) = compile_location(err) else {
        return theme_validation(path, err.message());
    };

    ThemeError::Parse {
        path: path.to_path_buf(),
        line,
        col,
        message: err.message().to_string(),
        excerpt: excerpt_at(source, line, col),
    }
}

/// Build a locationless theme validation error.
pub fn theme_validation(path: &Path, message: impl Into<String>) -> ThemeError {
    ThemeError::Validation {
        path: path.to_path_buf(),
        line: None,
        col: None,
        message: message.into(),
        excerpt: None,
    }
}

/// Convert a protected theme script failure into a located validation error.
pub fn theme_script_error<'s>(
    source: &str,
    path: &Path,
    scope: &Scope<'s>,
    err: &ScriptError<'s>,
) -> ThemeError {
    let message = script_error_message(scope, err, "theme script");
    let (line, col, excerpt) = theme_traceback_location(err.traceback(), path)
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

/// Convert a structured compile location into 1-based line and column coordinates.
fn compile_location(err: &CompileError) -> Option<(usize, usize)> {
    let location = err.location()?;
    Some((
        location.begin.line as usize + 1,
        location.begin.column as usize + 1,
    ))
}

/// Extract a readable message from a scoped script error value.
fn script_error_message<'s>(scope: &Scope<'s>, err: &ScriptError<'s>, noun: &str) -> String {
    from_scoped_value::<String>(scope, err.value())
        .unwrap_or_else(|_| format!("{noun} raised a {} value", err.value().type_name()))
}

/// Extract the VM's first rendered traceback line, falling back to a generic message.
fn protected_error_message(err: &ProtectedScriptError) -> String {
    err.traceback()
        .and_then(|traceback| traceback.lines().next())
        .unwrap_or("script raised an error")
        .to_string()
}

/// Extract a chunk name and line from one structured traceback frame.
fn traceback_frame_location(frame: &TracebackFrame) -> Option<(String, usize)> {
    frame
        .line
        .map(|line| (frame.chunk_name.clone(), line as usize))
}

/// Extract the first `path:line` location from an ruau traceback.
fn first_traceback_location(traceback: Option<&str>) -> Option<(String, usize)> {
    traceback?
        .lines()
        .find_map(|line| parse_location_prefix(line.trim()))
}

/// Parse a single traceback line of the form `path:line: ...`.
fn parse_location_prefix(line: &str) -> Option<(String, usize)> {
    for (index, ch) in line.char_indices() {
        if ch != ':' {
            continue;
        }
        let rest = &line[index + 1..];
        let digits = rest
            .chars()
            .take_while(char::is_ascii_digit)
            .collect::<String>();
        if digits.is_empty() {
            continue;
        }
        let line_no = digits.parse::<usize>().ok()?;
        return Some((line[..index].to_string(), line_no));
    }
    None
}

/// Extract the first traceback line that matches one theme path.
fn theme_traceback_location(traceback: Option<&str>, path: &Path) -> Option<(usize, usize)> {
    let expected = path.to_string_lossy();
    traceback?.lines().find_map(|line| {
        let (found, line) = parse_location_prefix(line.trim())?;
        (found == expected).then_some((line, 1))
    })
}

/// Convert VM traceback chunk names into user-facing filesystem paths.
fn normalize_chunk_path(path: String, default_path: Option<PathBuf>) -> Option<PathBuf> {
    match path.as_str() {
        "<memory>" => None,
        value if value.starts_with("[string ") => default_path,
        _ => Some(PathBuf::from(path)),
    }
}

/// Render an excerpt from either the source map or the in-memory root source.
fn config_source_excerpt(
    memory_source: Option<&str>,
    sources: &SourceMap,
    path: Option<&PathBuf>,
    line: usize,
    col: usize,
) -> Option<String> {
    match path {
        Some(path) => lock_unpoisoned(sources)
            .get(path)
            .map(|source| excerpt_at(source.as_ref(), line, col)),
        None => memory_source
            .map(|source| excerpt_at(source, line, col))
            .or_else(|| {
                lock_unpoisoned(sources)
                    .get(&PathBuf::from("<memory>"))
                    .map(|source| excerpt_at(source.as_ref(), line, col))
            }),
    }
}

/// Convert a byte offset into 1-based line and column coordinates.
fn line_col_at(source: &str, offset: usize) -> (usize, usize) {
    let clamped = offset.min(source.len());
    let prefix = &source[..clamped];
    let line = prefix.bytes().filter(|byte| *byte == b'\n').count() + 1;
    let col = prefix
        .rsplit_once('\n')
        .map_or(prefix.chars().count() + 1, |(_, tail)| {
            tail.chars().count() + 1
        });
    (line, col)
}

/// Render checker diagnostics using user-source line numbers instead of prelude offsets.
fn render_type_diagnostics(
    path: &Path,
    diagnostics: &[TypeDiagnostic],
    line_offset: usize,
) -> String {
    diagnostics
        .iter()
        .map(|diagnostic| {
            let site = source_position(diagnostic.primary_location, line_offset).map_or_else(
                || format!("{}:?:?", path.display()),
                |(line, col)| format!("{}:{}:{}", path.display(), line, col),
            );
            format!(
                "{} {}: {}",
                site,
                diagnostic.category,
                diagnostic
                    .context
                    .as_deref()
                    .unwrap_or("type checker diagnostic")
            )
        })
        .collect::<Vec<_>>()
        .join("; ")
}

/// Convert a checker source location into 1-based line and column coordinates.
fn source_position(location: DiagnosticLocation, line_offset: usize) -> Option<(usize, usize)> {
    if location == DiagnosticLocation::missing() {
        None
    } else {
        let line = location.begin.line as usize + 1;
        (line > line_offset).then_some((line - line_offset, location.begin.column as usize + 1))
    }
}
