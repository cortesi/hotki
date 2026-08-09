//! Complete repository test gate.

use std::path::Path;

use crate::{
    Result,
    cmd::{OutputMode, run_status},
    luau,
};

/// Run Luau validation, Rust tests, and the native smoketest.
pub fn test(root_dir: &Path) -> Result<()> {
    luau::luau(root_dir)?;

    println!("==> cargo test --locked --all");
    run_status(
        root_dir,
        "cargo",
        ["test", "--locked", "--all"],
        OutputMode::Streaming,
    )?;

    println!("==> cargo run --locked --bin smoketest -- all");
    run_status(
        root_dir,
        "cargo",
        ["run", "--locked", "--bin", "smoketest", "--", "all"],
        OutputMode::Streaming,
    )
}
