//! Transactional screenshot gallery generation.

use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsString,
    fs,
    io::{Error as IoError, Read},
    path::Path,
};

use crate::{
    Error, Result, artifact,
    cmd::{OutputMode, run_status},
};

/// Directory containing the generated README screenshot gallery.
const SCREENSHOT_DIR: &str = "assets/screenshots";
/// Repository-local staging root for generated artifacts.
const SCREENSHOT_STAGE_ROOT: &str = "tmp";
/// Checked-in config fixture used to capture the embedded default style.
const SCREENSHOT_CONFIG: &str = "crates/hotki-shots/fixtures/config.luau";
/// PNG signature and IHDR prefix length needed to read dimensions.
const PNG_HEADER_LEN: usize = 24;

/// Expected checked-in screenshot manifest and exact pixel dimensions.
const EXPECTED_SCREENSHOTS: [(&str, u32, u32); 6] = [
    ("hud.png", 480, 250),
    ("notify_error.png", 840, 156),
    ("notify_info.png", 840, 156),
    ("notify_success.png", 840, 156),
    ("notify_warning.png", 840, 156),
    ("selector.png", 960, 522),
];

/// Generate, verify, and atomically publish the default-style gallery.
pub fn screenshots(root_dir: &Path) -> Result<()> {
    println!("==> Capturing screenshots");
    let gallery = root_dir.join(SCREENSHOT_DIR);
    let stage_root = root_dir.join(SCREENSHOT_STAGE_ROOT);
    fs::create_dir_all(&stage_root).map_err(|source| Error::Io {
        path: stage_root.clone(),
        source,
    })?;
    let stage = artifact::unique_sibling(&stage_root.join("screenshots"), "capture")?;
    fs::create_dir(&stage).map_err(|source| Error::Io {
        path: stage.clone(),
        source,
    })?;

    let result = capture_and_publish(root_dir, &stage, &gallery);
    match result {
        Ok(()) => {
            if stage.exists()
                && let Err(source) = fs::remove_dir_all(&stage)
            {
                eprintln!(
                    "WARNING: screenshot gallery committed, but prior gallery cleanup failed at \
                     {}: {source}",
                    stage.display()
                );
            }
            Ok(())
        }
        Err(error) => {
            if stage.exists()
                && let Err(source) = fs::remove_dir_all(&stage)
            {
                eprintln!(
                    "WARNING: failed screenshot candidate remains at {}: {source}",
                    stage.display()
                );
            }
            Err(error)
        }
    }
}

/// Build and capture into `stage`, then publish only the verified complete set.
fn capture_and_publish(root_dir: &Path, stage: &Path, gallery: &Path) -> Result<()> {
    build_hotki_app(root_dir)?;
    run_status(
        root_dir,
        "cargo",
        [
            OsString::from("run"),
            OsString::from("--bin"),
            OsString::from("hotki-shots"),
            OsString::from("--"),
            OsString::from("--config"),
            root_dir.join(SCREENSHOT_CONFIG).into_os_string(),
            OsString::from("--dir"),
            stage.as_os_str().to_os_string(),
        ],
        OutputMode::Streaming,
    )?;
    verify_gallery(stage)?;
    publish_gallery(stage, gallery)
}

/// Build the GUI app binary used by the screenshot harness.
fn build_hotki_app(root_dir: &Path) -> Result<()> {
    run_status(
        root_dir,
        "cargo",
        [
            OsString::from("build"),
            OsString::from("-p"),
            OsString::from("hotki-app"),
            OsString::from("--bin"),
            OsString::from("hotki-app"),
        ],
        OutputMode::Streaming,
    )
}

/// Require the complete exact filename and dimension manifest.
fn verify_gallery(path: &Path) -> Result<()> {
    let expected = expected_manifest();
    let observed = fs::read_dir(path)
        .map_err(|source| Error::Io {
            path: path.to_path_buf(),
            source,
        })?
        .map(|entry| {
            entry
                .map_err(|source| Error::Io {
                    path: path.to_path_buf(),
                    source,
                })
                .and_then(|entry| {
                    entry.file_name().into_string().map_err(|name| Error::Io {
                        path: entry.path(),
                        source: IoError::other(format!(
                            "screenshot filename is not UTF-8: {name:?}"
                        )),
                    })
                })
        })
        .collect::<Result<BTreeSet<_>>>()?;
    let wanted = expected.keys().copied().collect::<BTreeSet<_>>();
    let observed_names = observed.iter().map(String::as_str).collect::<BTreeSet<_>>();
    if observed_names != wanted {
        return Err(Error::Io {
            path: path.to_path_buf(),
            source: IoError::other(format!(
                "screenshot manifest mismatch: expected {wanted:?}, observed {observed_names:?}"
            )),
        });
    }

    for (name, dimensions) in expected {
        let image = path.join(name);
        let observed_dimensions = read_png_dimensions(&image)?;
        if observed_dimensions != dimensions {
            return Err(Error::Io {
                path: image,
                source: IoError::other(format!(
                    "expected {}x{}, observed {}x{}",
                    dimensions.0, dimensions.1, observed_dimensions.0, observed_dimensions.1
                )),
            });
        }
    }
    Ok(())
}

/// Return the filename-to-dimensions gallery contract.
fn expected_manifest() -> BTreeMap<&'static str, (u32, u32)> {
    EXPECTED_SCREENSHOTS
        .into_iter()
        .map(|(name, width, height)| (name, (width, height)))
        .collect()
}

/// Read PNG dimensions directly from the required IHDR header.
fn read_png_dimensions(path: &Path) -> Result<(u32, u32)> {
    let mut header = [0_u8; PNG_HEADER_LEN];
    let mut file = fs::File::open(path).map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source,
    })?;
    file.read_exact(&mut header).map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source,
    })?;
    if header[..8] != *b"\x89PNG\r\n\x1a\n" || header[12..16] != *b"IHDR" {
        return Err(Error::Io {
            path: path.to_path_buf(),
            source: IoError::other("file does not contain a PNG IHDR header"),
        });
    }
    let width = u32::from_be_bytes(header[16..20].try_into().expect("fixed PNG width slice"));
    let height = u32::from_be_bytes(header[20..24].try_into().expect("fixed PNG height slice"));
    Ok((width, height))
}

/// Atomically replace the gallery, leaving the prior gallery at `stage` for cleanup.
fn publish_gallery(stage: &Path, gallery: &Path) -> Result<()> {
    if gallery.exists() {
        artifact::exchange_paths(stage, gallery)
    } else {
        artifact::rename_path(stage, gallery)
    }
}

#[cfg(test)]
mod tests {
    use std::{
        path::PathBuf,
        process,
        sync::atomic::{AtomicU64, Ordering},
    };

    use super::*;

    static NEXT_TEST: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn gallery_verification_accepts_only_the_exact_manifest() {
        let root = test_dir("manifest");
        write_gallery(&root);
        verify_gallery(&root).expect("verify expected gallery");

        fs::write(root.join("extra.png"), b"extra").expect("write extra file");
        assert!(verify_gallery(&root).is_err());
        fs::remove_dir_all(root).expect("remove gallery test");
    }

    #[test]
    fn gallery_verification_rejects_wrong_dimensions() {
        let root = test_dir("dimensions");
        write_gallery(&root);
        write_png_header(&root.join("hud.png"), 1, 1);

        assert!(verify_gallery(&root).is_err());
        fs::remove_dir_all(root).expect("remove gallery test");
    }

    fn test_dir(label: &str) -> PathBuf {
        let nonce = NEXT_TEST.fetch_add(1, Ordering::Relaxed);
        let path = PathBuf::from("tmp").join(format!(
            "xtask-screenshots-{label}-{}-{nonce}",
            process::id()
        ));
        if path.exists() {
            fs::remove_dir_all(&path).expect("remove stale test directory");
        }
        fs::create_dir_all(&path).expect("create test directory");
        path
    }

    fn write_gallery(path: &Path) {
        for (name, width, height) in EXPECTED_SCREENSHOTS {
            write_png_header(&path.join(name), width, height);
        }
    }

    fn write_png_header(path: &Path, width: u32, height: u32) {
        let mut header = [0_u8; PNG_HEADER_LEN];
        header[..8].copy_from_slice(b"\x89PNG\r\n\x1a\n");
        header[8..12].copy_from_slice(&13_u32.to_be_bytes());
        header[12..16].copy_from_slice(b"IHDR");
        header[16..20].copy_from_slice(&width.to_be_bytes());
        header[20..24].copy_from_slice(&height.to_be_bytes());
        fs::write(path, header).expect("write PNG header");
    }
}
