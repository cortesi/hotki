//! Screenshot generation tasks.

use std::path::Path;

use crate::{Result, cmd::run_status_streaming};

/// Generate screenshots for all built-in themes.
pub fn screenshots(root_dir: &Path) -> Result<()> {
    let themes = [
        ("default", "assets/default"),
        ("solarized-dark", "assets/solarized-dark"),
        ("solarized-light", "assets/solarized-light"),
        ("dark-blue", "assets/dark-blue"),
        ("charcoal", "assets/charcoal"),
    ];

    for (theme, out_dir) in themes {
        println!("==> Capturing screenshots: {theme}");
        run_status_streaming(
            root_dir,
            "cargo",
            [
                "run",
                "--bin",
                "hotki-shots",
                "--",
                "--theme",
                theme,
                "--dir",
                out_dir,
            ],
        )?;
    }

    Ok(())
}
