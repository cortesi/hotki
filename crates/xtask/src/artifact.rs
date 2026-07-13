//! Atomic macOS artifact publication helpers.

use std::{
    ffi::CString,
    fs,
    io::{self, Error as IoError, ErrorKind},
    os::unix::ffi::OsStrExt,
    path::{Path, PathBuf},
    process,
    sync::atomic::{AtomicU64, Ordering},
};

use crate::{Error, Result};

/// Process-local suffix for unique sibling artifact paths.
static NEXT_PATH: AtomicU64 = AtomicU64::new(0);

/// Return a unique hidden sibling path suitable for same-filesystem publication.
pub fn unique_sibling(path: &Path, label: &str) -> Result<PathBuf> {
    let parent = path.parent().ok_or_else(|| Error::Io {
        path: path.to_path_buf(),
        source: IoError::other("artifact path has no parent"),
    })?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| Error::Io {
            path: path.to_path_buf(),
            source: IoError::other("artifact path has no UTF-8 file name"),
        })?;
    loop {
        let nonce = NEXT_PATH.fetch_add(1, Ordering::Relaxed);
        let candidate = parent.join(format!(".{name}.{label}-{}-{nonce}", process::id()));
        match fs::symlink_metadata(&candidate) {
            Ok(_) => continue,
            Err(source) if source.kind() == ErrorKind::NotFound => return Ok(candidate),
            Err(source) => {
                return Err(Error::Io {
                    path: candidate,
                    source,
                });
            }
        }
    }
}

/// Atomically exchange two existing paths on macOS.
pub fn exchange_paths(left: &Path, right: &Path) -> Result<()> {
    let left_c = path_c_string(left)?;
    let right_c = path_c_string(right)?;
    let result = unsafe { libc::renamex_np(left_c.as_ptr(), right_c.as_ptr(), libc::RENAME_SWAP) };
    if result == 0 {
        return Ok(());
    }
    Err(Error::Io {
        path: right.to_path_buf(),
        source: io::Error::last_os_error(),
    })
}

/// Rename one path and retain destination context in any error.
pub fn rename_path(source_path: &Path, destination: &Path) -> Result<()> {
    fs::rename(source_path, destination).map_err(|source| Error::Io {
        path: destination.to_path_buf(),
        source,
    })
}

/// Convert a filesystem path into the C string required by `renamex_np`.
fn path_c_string(path: &Path) -> Result<CString> {
    CString::new(path.as_os_str().as_bytes()).map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source: IoError::new(io::ErrorKind::InvalidInput, source),
    })
}

#[cfg(test)]
mod tests {
    use std::{fs, process, sync::atomic::AtomicU64};

    use super::*;

    static NEXT_TEST: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn exchange_paths_swaps_complete_artifacts() {
        let nonce = NEXT_TEST.fetch_add(1, Ordering::Relaxed);
        let root = PathBuf::from("tmp").join(format!("xtask-artifact-{}-{nonce}", process::id()));
        let left = root.join("left");
        let right = root.join("right");
        fs::create_dir_all(&left).expect("create left");
        fs::create_dir_all(&right).expect("create right");
        fs::write(left.join("value"), "left").expect("write left");
        fs::write(right.join("value"), "right").expect("write right");

        exchange_paths(&left, &right).expect("exchange artifacts");

        assert_eq!(fs::read_to_string(left.join("value")).unwrap(), "right");
        assert_eq!(fs::read_to_string(right.join("value")).unwrap(), "left");
        fs::remove_dir_all(root).expect("remove test artifacts");
    }
}
