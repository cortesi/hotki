//! macOS install task for Hotki.

use std::{
    env::consts,
    fs,
    io::Error as IoError,
    path::{Path, PathBuf},
};

use crate::{
    Error, Result, bundle,
    cmd::{OutputMode, run_status},
};

/// Standard system Applications directory on macOS.
const APPLICATIONS_DIR: &str = "/Applications";

/// Build and install Hotki to `/Applications`.
pub fn install(root_dir: &Path) -> Result<()> {
    ensure_macos()?;

    println!("==> Building Hotki release bundle");
    let source_bundle = bundle::bundle_release_default(root_dir)?;
    let installed_bundle = applications_bundle_path(&source_bundle)?;

    println!(
        "==> Installing app bundle into {}",
        installed_bundle.display()
    );
    remove_existing_install(&installed_bundle)?;
    run_status(
        root_dir,
        "mv",
        [source_bundle.as_os_str(), installed_bundle.as_os_str()],
        OutputMode::Streaming,
    )?;

    println!("==> Install complete: {}", installed_bundle.display());
    Ok(())
}

/// Return an error when the current target is not macOS.
fn ensure_macos() -> Result<()> {
    if cfg!(target_os = "macos") {
        Ok(())
    } else {
        Err(Error::Unsupported(format!(
            "xtask install is only supported on macOS (target: {})",
            consts::OS
        )))
    }
}

/// Resolve the destination `.app` path under `/Applications`.
fn applications_bundle_path(source_bundle: &Path) -> Result<PathBuf> {
    let file_name = source_bundle.file_name().ok_or_else(|| Error::Io {
        path: source_bundle.to_path_buf(),
        source: IoError::other("invalid bundle path (missing file name)"),
    })?;
    Ok(Path::new(APPLICATIONS_DIR).join(file_name))
}

/// Remove an existing installed app bundle before replacement.
fn remove_existing_install(path: &Path) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    if path.is_dir() {
        fs::remove_dir_all(path).map_err(|source| Error::Io {
            path: path.to_path_buf(),
            source,
        })?;
    } else {
        fs::remove_file(path).map_err(|source| Error::Io {
            path: path.to_path_buf(),
            source,
        })?;
    }
    Ok(())
}
