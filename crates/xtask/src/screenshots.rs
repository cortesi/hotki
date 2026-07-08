//! Screenshot generation tasks.

use std::{ffi::OsString, fs, path::Path};

use crate::{
    Error, Result,
    cmd::{OutputMode, run_status},
};

/// Directory containing the generated README screenshot gallery.
const SCREENSHOT_DIR: &str = "assets/screenshots";
/// Checked-in config fixture used to capture the embedded default style.
const SCREENSHOT_CONFIG: &str = "crates/hotki-shots/fixtures/config.luau";

/// Generate screenshots for the default style.
pub fn screenshots(root_dir: &Path) -> Result<()> {
    println!("==> Capturing screenshots");
    let screenshot_dir = root_dir.join(SCREENSHOT_DIR);
    recreate_dir(&screenshot_dir)?;
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
            screenshot_dir.into_os_string(),
        ],
        OutputMode::Streaming,
    )?;

    Ok(())
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

/// Remove and recreate a generated artifact directory.
fn recreate_dir(path: &Path) -> Result<()> {
    if path.exists() {
        fs::remove_dir_all(path).map_err(|source| Error::Io {
            path: path.to_path_buf(),
            source,
        })?;
    }
    fs::create_dir_all(path).map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(())
}
