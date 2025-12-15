//! Error types for configuration loading and validation.

use std::{
    cmp::{max, min},
    fmt::Write as _,
    path::{Path, PathBuf},
};

use thiserror::Error;

#[derive(Debug, Error, Clone)]
/// Errors produced while loading, parsing, or validating a configuration.
pub enum Error {
    #[error("{message}")]
    /// I/O or filesystem read error.
    Read {
        /// Optional path associated with the read error.
        path: Option<PathBuf>,
        /// Human-readable error message.
        message: String,
    },
    #[error("{message}")]
    /// Rhai parse error with a concrete line/column location and excerpt.
    Parse {
        /// Optional path associated with the parse error.
        path: Option<PathBuf>,
        /// 1-based line number.
        line: usize,
        /// 1-based column number.
        col: usize,
        /// Human-readable error message.
        message: String,
        /// Rendered excerpt including a caret at the error location.
        excerpt: String,
    },
    #[error("{message}")]
    /// Validation or runtime error, optionally including location and excerpt.
    Validation {
        /// Optional path associated with the validation error.
        path: Option<PathBuf>,
        /// Optional 1-based line number.
        line: Option<usize>,
        /// Optional 1-based column number.
        col: Option<usize>,
        /// Human-readable error message.
        message: String,
        /// Optional excerpt including a caret at the error location.
        excerpt: Option<String>,
    },
}

impl Error {
    /// Render a human-friendly error message including location and an excerpt when available.
    pub fn pretty(&self) -> String {
        match self {
            Self::Read { path, message } => match path {
                Some(p) => format!("Read error at {}: {}", p.display(), message),
                None => format!("Read error: {}", message),
            },
            Self::Parse {
                path,
                line,
                col,
                message,
                excerpt,
            } => match path {
                Some(p) => format!(
                    "Config parse error at {}:{}:{}\n{}\n{}",
                    p.display(),
                    line,
                    col,
                    message,
                    excerpt
                ),
                None => format!(
                    "Config parse error at line {}, column {}\n{}\n{}",
                    line, col, message, excerpt
                ),
            },
            Self::Validation {
                path,
                line,
                col,
                message,
                excerpt,
            } => {
                let loc = match (line, col) {
                    (Some(l), Some(c)) => format!("{}:{}", l, c),
                    (Some(l), None) => format!("{}", l),
                    _ => String::new(),
                };
                match (path, excerpt) {
                    (Some(p), Some(ex)) if !loc.is_empty() => format!(
                        "Config validation error at {}:{}\n{}\n{}",
                        p.display(),
                        loc,
                        message,
                        ex
                    ),
                    (Some(p), _) if !loc.is_empty() => format!(
                        "Config validation error at {}:{}\n{}",
                        p.display(),
                        loc,
                        message
                    ),
                    (Some(p), _) => {
                        format!("Config validation error at {}\n{}", p.display(), message)
                    }
                    (None, Some(ex)) if !loc.is_empty() => {
                        format!("Config validation error at {}\n{}\n{}", loc, message, ex)
                    }
                    (None, _) if !loc.is_empty() => {
                        format!("Config validation error at {}\n{}", loc, message)
                    }
                    (None, _) => format!("Config validation error\n{}", message),
                }
            }
        }
    }

    /// Access the optional path attached to this error.
    pub fn path(&self) -> Option<&Path> {
        match self {
            Self::Read { path, .. } | Self::Parse { path, .. } | Self::Validation { path, .. } => {
                path.as_deref()
            }
        }
    }
}

/// Build a small 2–3 line excerpt with a caret at `(line_no, col_no)`.
pub fn excerpt_at(source: &str, line_no: usize, col_no: usize) -> String {
    let lines: Vec<&str> = source.lines().collect();
    let total = lines.len();
    let start = max(1usize, line_no.saturating_sub(2));
    let end = min(total, line_no + 1);

    let mut out = String::new();
    for n in start..=end {
        let text = lines.get(n - 1).copied().unwrap_or("");
        let _ignored = writeln!(out, " {:>4} | {}", n, text);
        if n == line_no {
            let prefix = format!(" {:>4} | ", n);
            let _ignored = writeln!(
                out,
                "{}{}^",
                " ".repeat(prefix.len()),
                " ".repeat(col_no.saturating_sub(1))
            );
        }
    }
    out
}
