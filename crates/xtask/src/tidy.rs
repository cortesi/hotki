//! Linting and formatting tasks.

use std::path::Path;

use crate::{Result, cmd::run_status_streaming};

/// Run workspace lint and format checks.
pub fn tidy(root_dir: &Path) -> Result<()> {
    println!("==> cargo clippy --fix");
    run_status_streaming(
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
    )?;

    println!("==> cargo fmt");
    if root_dir.join("rustfmt-nightly.toml").is_file() {
        run_status_streaming(
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
        )?;
    } else {
        run_status_streaming(root_dir, "cargo", ["+nightly", "fmt", "--all"])?;
    }

    Ok(())
}
