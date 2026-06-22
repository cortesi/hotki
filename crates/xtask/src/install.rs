//! macOS install task for Hotki.

use std::{
    fs,
    io::Error as IoError,
    path::{Path, PathBuf},
};

use clap::Args;

use crate::{
    Error, Result, bundle,
    cmd::{OutputMode, run_status},
};

/// Standard system Applications directory on macOS.
const APPLICATIONS_DIR: &str = "/Applications";

/// Arguments for `cargo xtask install`.
#[derive(Debug, Args)]
pub struct InstallArgs {
    /// Replace the current app without updating the `.bak` backup.
    #[arg(long)]
    no_backup: bool,
}

/// Build and install Hotki to `/Applications`.
pub fn install(root_dir: &Path, args: &InstallArgs) -> Result<()> {
    println!("==> Building Hotki release bundle");
    let source_bundle = bundle::bundle_release_default(root_dir)?;
    let installed_bundle = applications_bundle_path(&source_bundle)?;
    let backup_bundle = backup_bundle_path(&installed_bundle)?;

    if args.no_backup {
        println!(
            "==> Backup rotation disabled; leaving {} unchanged",
            backup_bundle.display()
        );
        if installed_bundle.exists() {
            println!(
                "==> Removing current install {}",
                installed_bundle.display()
            );
            remove_existing_path(&installed_bundle)?;
        } else {
            println!(
                "==> No current install found at {}; nothing to overwrite",
                installed_bundle.display()
            );
        }
    } else {
        println!("==> Removing previous backup {}", backup_bundle.display());
        remove_existing_path(&backup_bundle)?;
        if installed_bundle.exists() {
            println!(
                "==> Backing up current install {} -> {}",
                installed_bundle.display(),
                backup_bundle.display()
            );
            move_path(root_dir, &installed_bundle, &backup_bundle)?;
        } else {
            println!(
                "==> No current install found at {}; skipping backup",
                installed_bundle.display()
            );
        }
    }

    println!(
        "==> Installing new app bundle to {}",
        installed_bundle.display()
    );
    move_path(root_dir, &source_bundle, &installed_bundle)?;

    println!("==> Install complete: {}", installed_bundle.display());
    Ok(())
}

/// Resolve the destination `.app` path under `/Applications`.
fn applications_bundle_path(source_bundle: &Path) -> Result<PathBuf> {
    let file_name = source_bundle.file_name().ok_or_else(|| Error::Io {
        path: source_bundle.to_path_buf(),
        source: IoError::other("invalid bundle path (missing file name)"),
    })?;
    Ok(Path::new(APPLICATIONS_DIR).join(file_name))
}

/// Return the backup path for an installed app bundle.
fn backup_bundle_path(installed_bundle: &Path) -> Result<PathBuf> {
    let file_name = installed_bundle
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| Error::Io {
            path: installed_bundle.to_path_buf(),
            source: IoError::other("invalid installed bundle path (missing file name)"),
        })?;
    Ok(installed_bundle.with_file_name(format!("{file_name}.bak")))
}

/// Remove an existing file or directory path.
fn remove_existing_path(path: &Path) -> Result<()> {
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

/// Move a path using the platform `mv` command.
fn move_path(root_dir: &Path, src: &Path, dst: &Path) -> Result<()> {
    run_status(
        root_dir,
        "mv",
        [src.as_os_str(), dst.as_os_str()],
        OutputMode::Streaming,
    )
}
