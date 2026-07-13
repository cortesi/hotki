//! Transactional macOS installation for Hotki.

use std::{
    env, fs,
    io::{Error as IoError, ErrorKind},
    os::unix::fs::symlink,
    path::{Path, PathBuf},
};

use clap::Args;

use crate::{
    Error, Result, artifact, bundle,
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

/// Build, stage, verify, and install Hotki under `/Applications`.
pub fn install(root_dir: &Path, args: &InstallArgs) -> Result<()> {
    println!("==> Building Hotki release bundle");
    let source_bundle = bundle::bundle_release_default(root_dir)?;
    bundle::verify_release_bundle(&source_bundle)?;
    let installed_bundle = applications_bundle_path(&source_bundle)?;
    let backup_bundle = backup_bundle_path(&installed_bundle)?;
    let cli_link = (!args.no_cli_link)
        .then(|| preflight_cli_link(args.cli_link.as_deref()))
        .transpose()?;

    let staged_bundle = artifact::unique_sibling(&installed_bundle, "stage")?;
    println!("==> Staging app beside destination");
    copy_bundle(root_dir, &source_bundle, &staged_bundle)?;
    bundle::verify_release_bundle(&staged_bundle)?;

    let outcome = commit_staged_bundle(
        &staged_bundle,
        &installed_bundle,
        &backup_bundle,
        args.no_backup,
        bundle::verify_release_bundle,
    )?;
    if let Some(recovery_path) = outcome.recovery_path {
        eprintln!(
            "WARNING: install committed, but a prior artifact remains at {}",
            recovery_path.display()
        );
    }

    if let Some(link_path) = cli_link {
        publish_cli_link(&installed_bundle, &link_path)?;
    }
    println!("==> Install complete: {}", installed_bundle.display());
    Ok(())
}

/// Result of publishing and verifying a staged application bundle.
struct CommitOutcome {
    /// Prior artifact retained because post-commit retirement did not complete.
    recovery_path: Option<PathBuf>,
}

/// Atomically publish `staged`, verify readback, and retain or remove the prior app.
fn commit_staged_bundle<F>(
    staged: &Path,
    installed: &Path,
    backup: &Path,
    no_backup: bool,
    verify: F,
) -> Result<CommitOutcome>
where
    F: Fn(&Path) -> Result<()>,
{
    verify(staged)?;
    if !installed.exists() {
        artifact::rename_path(staged, installed)?;
        if let Err(error) = verify(installed) {
            if let Err(rollback_error) = artifact::rename_path(installed, staged) {
                return Err(rollback_failure(staged, &error, &rollback_error));
            }
            return Err(error);
        }
        return Ok(CommitOutcome {
            recovery_path: None,
        });
    }

    artifact::exchange_paths(staged, installed)?;
    if let Err(error) = verify(installed) {
        if let Err(rollback_error) = artifact::exchange_paths(staged, installed) {
            return Err(rollback_failure(staged, &error, &rollback_error));
        }
        return Err(error);
    }

    Ok(CommitOutcome {
        recovery_path: retire_prior_bundle(staged, backup, no_backup),
    })
}

/// Retire the prior installed bundle after the new installation has committed.
fn retire_prior_bundle(staged: &Path, backup: &Path, no_backup: bool) -> Option<PathBuf> {
    if no_backup {
        return remove_existing_path(staged)
            .err()
            .map(|_error| staged.to_path_buf());
    }
    if backup.exists() {
        if artifact::exchange_paths(staged, backup).is_err() {
            return Some(staged.to_path_buf());
        }
        return remove_existing_path(staged)
            .err()
            .map(|_error| staged.to_path_buf());
    }
    artifact::rename_path(staged, backup)
        .err()
        .map(|_error| staged.to_path_buf())
}

/// Preserve both verification and rollback context when atomic recovery itself fails.
fn rollback_failure(staged: &Path, verify_error: &Error, rollback_error: &Error) -> Error {
    Error::Io {
        path: staged.to_path_buf(),
        source: IoError::other(format!(
            "installed bundle failed readback ({verify_error}); rollback also failed \
             ({rollback_error}); recovery artifact preserved"
        )),
    }
}

/// Copy a bundle with macOS metadata into a destination-side staging path.
fn copy_bundle(root_dir: &Path, source: &Path, staged: &Path) -> Result<()> {
    run_status(
        root_dir,
        "ditto",
        [source.as_os_str(), staged.as_os_str()],
        OutputMode::Streaming,
    )
}

/// Resolve the CLI link and reject unsafe replacements before app mutation.
fn preflight_cli_link(explicit_path: Option<&Path>) -> Result<PathBuf> {
    let link_path = cli_link_path(explicit_path)?;
    let parent = link_path.parent().ok_or_else(|| Error::Io {
        path: link_path.clone(),
        source: IoError::other("invalid CLI link path (missing parent)"),
    })?;
    fs::create_dir_all(parent).map_err(|source| Error::Io {
        path: parent.to_path_buf(),
        source,
    })?;
    match fs::symlink_metadata(&link_path) {
        Ok(metadata) if metadata.file_type().is_symlink() => Ok(link_path),
        Ok(_) => Err(Error::Io {
            path: link_path,
            source: IoError::new(
                ErrorKind::AlreadyExists,
                "CLI link path already exists and is not a symlink",
            ),
        }),
        Err(source) if source.kind() == ErrorKind::NotFound => Ok(link_path),
        Err(source) => Err(Error::Io {
            path: link_path,
            source,
        }),
    }
}

/// Atomically replace the user-facing CLI link after the app readback succeeds.
fn publish_cli_link(installed_bundle: &Path, link_path: &Path) -> Result<()> {
    let target = installed_bundle
        .join("Contents")
        .join("MacOS")
        .join(bundle::CLI_BIN_NAME);
    let staged_link = artifact::unique_sibling(link_path, "stage")?;
    symlink(&target, &staged_link).map_err(|source| Error::Io {
        path: staged_link.clone(),
        source,
    })?;
    if let Err(source) = fs::rename(&staged_link, link_path) {
        remove_existing_path(&staged_link)?;
        return Err(Error::Io {
            path: link_path.to_path_buf(),
            source,
        });
    }
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

/// Remove an existing file, symlink, or directory path.
fn remove_existing_path(path: &Path) -> Result<()> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(source) if source.kind() == ErrorKind::NotFound => return Ok(()),
        Err(source) => {
            return Err(Error::Io {
                path: path.to_path_buf(),
                source,
            });
        }
    };
    if metadata.is_dir() && !metadata.file_type().is_symlink() {
        fs::remove_dir_all(path).map_err(|source| Error::Io {
            path: path.to_path_buf(),
            source,
        })
    } else {
        fs::remove_file(path).map_err(|source| Error::Io {
            path: path.to_path_buf(),
            source,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::{
        os::unix::fs::PermissionsExt,
        process,
        sync::atomic::{AtomicU64, Ordering},
    };

    use super::*;

    static NEXT_TEST: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn successful_swap_rotates_previous_app_to_backup() {
        let root = test_dir("success");
        let installed = root.join("Hotki.app");
        let staged = root.join("stage.app");
        let backup = root.join("Hotki.app.bak");
        write_bundle(&installed, "old");
        write_bundle(&staged, "new");

        commit_staged_bundle(
            &staged,
            &installed,
            &backup,
            false,
            bundle::verify_release_bundle,
        )
        .expect("commit bundle");

        assert_eq!(bundle_marker(&installed), "new");
        assert_eq!(bundle_marker(&backup), "old");
        assert!(!staged.exists());
        fs::remove_dir_all(root).expect("remove test install");
    }

    #[test]
    fn no_backup_keeps_existing_backup_unchanged() {
        let root = test_dir("no-backup");
        let installed = root.join("Hotki.app");
        let staged = root.join("stage.app");
        let backup = root.join("Hotki.app.bak");
        write_bundle(&installed, "old");
        write_bundle(&staged, "new");
        write_bundle(&backup, "prior-backup");

        commit_staged_bundle(
            &staged,
            &installed,
            &backup,
            true,
            bundle::verify_release_bundle,
        )
        .expect("commit bundle without backup");

        assert_eq!(bundle_marker(&installed), "new");
        assert_eq!(bundle_marker(&backup), "prior-backup");
        fs::remove_dir_all(root).expect("remove test install");
    }

    #[test]
    fn controlled_readback_failure_restores_previous_app() {
        let root = test_dir("rollback");
        let installed = root.join("Hotki.app");
        let staged = root.join("stage.app");
        let backup = root.join("Hotki.app.bak");
        write_bundle(&installed, "old");
        write_bundle(&staged, "new");
        write_bundle(&backup, "prior-backup");
        let installed_for_verify = installed.clone();

        let result = commit_staged_bundle(&staged, &installed, &backup, false, |path| {
            bundle::verify_release_bundle(path)?;
            if path == installed_for_verify && bundle_marker(path) == "new" {
                return Err(Error::Io {
                    path: path.to_path_buf(),
                    source: IoError::other("controlled readback failure"),
                });
            }
            Ok(())
        });

        assert!(result.is_err());
        assert_eq!(bundle_marker(&installed), "old");
        assert_eq!(bundle_marker(&staged), "new");
        assert_eq!(bundle_marker(&backup), "prior-backup");
        fs::remove_dir_all(root).expect("remove test install");
    }

    #[test]
    fn successful_commit_atomically_replaces_existing_backup() {
        let root = test_dir("existing-backup");
        let installed = root.join("Hotki.app");
        let staged = root.join("stage.app");
        let backup = root.join("Hotki.app.bak");
        write_bundle(&installed, "old");
        write_bundle(&staged, "new");
        write_bundle(&backup, "prior-backup");

        let outcome = commit_staged_bundle(
            &staged,
            &installed,
            &backup,
            false,
            bundle::verify_release_bundle,
        )
        .expect("commit bundle");

        assert!(outcome.recovery_path.is_none());
        assert_eq!(bundle_marker(&installed), "new");
        assert_eq!(bundle_marker(&backup), "old");
        assert!(!staged.exists());
        fs::remove_dir_all(root).expect("remove test install");
    }

    fn test_dir(label: &str) -> PathBuf {
        let nonce = NEXT_TEST.fetch_add(1, Ordering::Relaxed);
        let path =
            PathBuf::from("tmp").join(format!("xtask-install-{label}-{}-{nonce}", process::id()));
        if path.exists() {
            fs::remove_dir_all(&path).expect("remove stale test directory");
        }
        fs::create_dir_all(&path).expect("create test directory");
        path
    }

    fn write_bundle(path: &Path, marker: &str) {
        let macos = path.join("Contents/MacOS");
        let resources = path.join("Contents/Resources");
        fs::create_dir_all(&macos).expect("create MacOS directory");
        fs::create_dir_all(&resources).expect("create Resources directory");
        fs::write(path.join("Contents/Info.plist"), "plist").expect("write plist");
        for name in ["hotki-app", "hotki"] {
            let executable = macos.join(name);
            fs::write(&executable, name).expect("write executable");
            fs::set_permissions(&executable, fs::Permissions::from_mode(0o755))
                .expect("chmod executable");
        }
        fs::write(resources.join("hotki-app.icns"), "icon").expect("write icon");
        fs::write(path.join("marker"), marker).expect("write marker");
    }

    fn bundle_marker(path: &Path) -> String {
        fs::read_to_string(path.join("marker")).expect("read bundle marker")
    }
}
