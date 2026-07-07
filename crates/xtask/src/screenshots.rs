//! Screenshot generation tasks.

use std::path::Path;

use crate::{
    Result,
    cmd::{OutputMode, run_status},
};

/// Generate screenshots for the default style.
pub fn screenshots(root_dir: &Path) -> Result<()> {
    println!("==> Capturing screenshots");
    run_status(
        root_dir,
        "cargo",
        [
            "run",
            "--bin",
            "hotki-shots",
            "--",
            "--dir",
            "assets/default",
        ],
        OutputMode::Streaming,
    )?;

    Ok(())
}
