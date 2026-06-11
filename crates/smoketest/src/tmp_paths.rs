//! Repo-local temporary-path helpers for smoketest artifacts.

use std::{fs, path::PathBuf, process};

/// Repo-local root directory used for smoketest scratch files.
const TMP_ROOT: &str = "tmp";

/// Best-effort directory creation for UI helpers that already ignore I/O failures.
pub fn ensure_subdir_best_effort(name: &str) -> PathBuf {
    let dir = PathBuf::from(TMP_ROOT).join(name);
    if let Err(_err) = fs::create_dir_all(&dir) {}
    dir
}

/// Return a stable path for the current process under a repo-local temp subdirectory.
pub fn process_file_path(dir: &str, prefix: &str, label: &str, suffix: &str) -> PathBuf {
    ensure_subdir_best_effort(dir).join(format!("{prefix}-{label}-{}.{suffix}", process::id()))
}
