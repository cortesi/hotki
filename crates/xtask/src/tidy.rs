//! Linting and formatting tasks.

use std::path::Path;

use crate::{
    Result,
    cmd::{OutputMode, run_status},
};

/// Run workspace lint and format checks.
pub fn tidy(root_dir: &Path) -> Result<()> {
    println!("==> cargo clippy --fix");
    run_status(
        root_dir,
        "cargo",
        [
            "clippy",
            "-q",
            "--fix",
            "--all",
            "--all-targets",
            "--all-features",
            "--allow-dirty",
            "--tests",
            "--examples",
        ],
        OutputMode::Streaming,
    )?;

    println!("==> cargo fmt");
    if root_dir.join("rustfmt-nightly.toml").is_file() {
        run_status(
            root_dir,
            "cargo",
            [
                "+nightly",
                "fmt",
                "--all",
                "--",
                "--config-path",
                "./rustfmt-nightly.toml",
            ],
            OutputMode::Streaming,
        )?;
    } else {
        run_status(
            root_dir,
            "cargo",
            ["+nightly", "fmt", "--all"],
            OutputMode::Streaming,
        )?;
    }

    Ok(())
}
