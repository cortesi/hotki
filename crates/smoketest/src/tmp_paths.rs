//! Repo-local temporary-path helpers for smoketest artifacts.

use std::{
    fs,
    path::PathBuf,
    process,
    time::{SystemTime, UNIX_EPOCH},
};

use crate::error::Result;

/// Repo-local root directory used for smoketest scratch files.
const TMP_ROOT: &str = "tmp";

/// Ensure a named repo-local temp subdirectory exists.
pub fn ensure_subdir(name: &str) -> Result<PathBuf> {
    let dir = PathBuf::from(TMP_ROOT).join(name);
    fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// Best-effort version of [`ensure_subdir`] for UI helpers that already ignore I/O failures.
pub fn ensure_subdir_best_effort(name: &str) -> PathBuf {
    let dir = PathBuf::from(TMP_ROOT).join(name);
    if let Err(_err) = fs::create_dir_all(&dir) {}
    dir
}

/// Allocate a unique socket path under a repo-local temp subdirectory.
pub fn unique_socket_path(dir: &str, prefix: &str) -> Result<PathBuf> {
    Ok(ensure_subdir(dir)?.join(format!("{prefix}-{}-{}.sock", process::id(), now_nanos())))
}

/// Return a stable path for the current process under a repo-local temp subdirectory.
pub fn process_file_path(dir: &str, prefix: &str, label: &str, suffix: &str) -> PathBuf {
    ensure_subdir_best_effort(dir).join(format!("{prefix}-{label}-{}.{suffix}", process::id()))
}

/// Return a specific filename inside a repo-local temp subdirectory.
#[cfg(test)]
pub fn named_path(dir: &str, name: &str) -> Result<PathBuf> {
    Ok(ensure_subdir(dir)?.join(name))
}

/// Return a nanosecond timestamp used to keep temp filenames unique.
fn now_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    #[test]
    fn unique_socket_path_uses_repo_tmp() {
        let path = unique_socket_path("smoketest-tests", "bridge").unwrap();
        assert!(path.starts_with(Path::new("tmp").join("smoketest-tests")));
        assert_eq!(path.extension().and_then(|ext| ext.to_str()), Some("sock"));
    }
}
