//! Error types for configuration loading and validation.

use std::{
    cmp::{max, min},
    fmt::Write as _,
    path::{Path, PathBuf},
};

use thiserror::Error;

#[derive(Debug, Error, Clone)]
pub enum Error {
    #[error("{message}")]
    Read {
        path: Option<PathBuf>,
        message: String,
    },
    #[error("{message}")]
    Parse {
        path: Option<PathBuf>,
        line: usize,
        col: usize,
        message: String,
        excerpt: String,
    },
    #[error("{message}")]
    Validation {
        path: Option<PathBuf>,
        line: Option<usize>,
        col: Option<usize>,
        message: String,
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
