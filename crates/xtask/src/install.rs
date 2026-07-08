//! macOS install task for Hotki.

use std::{
    env, fs,
    io::{Error as IoError, ErrorKind},
    os::unix::fs::symlink,
    path::{Path, PathBuf},
};

use clap::Args;

use crate::{
    Error, Result, bundle,
    cmd::{OutputMode, run_status},
};

/// Standard system Applications directory on macOS.
const APPLICATIONS_DIR: &str = "/Applications";
/// Default user-local CLI link path, relative to the home directory.
const DEFAULT_CLI_LINK: &str = ".local/bin/hotki";

/// Arguments for `cargo xtask install`.
#[derive(Debug, Args)]
pub struct InstallArgs {
    /// Replace the current app without updating the `.bak` backup.
    #[arg(long)]
    no_backup: bool,
    /// Do not create or update a CLI symlink.
    #[arg(long)]
    no_cli_link: bool,
    /// Path for the installed CLI symlink.
    #[arg(long, value_name = "PATH")]
    cli_link: Option<PathBuf>,
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
    if !args.no_cli_link {
        install_cli_link(&installed_bundle, args.cli_link.as_deref())?;
    }
    Ok(())
}

/// Create or replace the user-facing CLI symlink.
fn install_cli_link(installed_bundle: &Path, explicit_path: Option<&Path>) -> Result<()> {
    let link_path = cli_link_path(explicit_path)?;
    let target = installed_bundle
        .join("Contents")
        .join("MacOS")
        .join(bundle::CLI_BIN_NAME);
    let parent = link_path.parent().ok_or_else(|| Error::Io {
        path: link_path.clone(),
        source: IoError::other("invalid CLI link path (missing parent)"),
    })?;
    fs::create_dir_all(parent).map_err(|source| Error::Io {
        path: parent.to_path_buf(),
        source,
    })?;
    remove_existing_cli_link(&link_path)?;
    symlink(&target, &link_path).map_err(|source| Error::Io {
        path: link_path.clone(),
        source,
    })?;
    println!("==> CLI available at {}", link_path.display());
    Ok(())
}

/// Resolve the CLI symlink path.
fn cli_link_path(explicit_path: Option<&Path>) -> Result<PathBuf> {
    if let Some(path) = explicit_path {
        return Ok(path.to_path_buf());
    }
    let home = env::var_os("HOME").ok_or_else(|| Error::Io {
        path: PathBuf::from(DEFAULT_CLI_LINK),
        source: IoError::other("HOME is not set; pass --cli-link or --no-cli-link"),
    })?;
    Ok(PathBuf::from(home).join(DEFAULT_CLI_LINK))
}

/// Remove an existing CLI link if it is safe to replace.
fn remove_existing_cli_link(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            fs::remove_file(path).map_err(|source| Error::Io {
                path: path.to_path_buf(),
                source,
            })?;
            Ok(())
        }
        Ok(_) => Err(Error::Io {
            path: path.to_path_buf(),
            source: IoError::new(
                ErrorKind::AlreadyExists,
                "CLI link path already exists and is not a symlink",
            ),
        }),
        Err(source) if source.kind() == ErrorKind::NotFound => Ok(()),
        Err(source) => Err(Error::Io {
            path: path.to_path_buf(),
            source,
        }),
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
