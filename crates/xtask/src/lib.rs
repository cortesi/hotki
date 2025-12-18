#![warn(missing_docs)]
//! Internal build and development tasks for the Hotki workspace.

use clap::{Parser, Subcommand};

/// `.app` bundle build tasks.
mod bundle;
/// Utilities for running external commands.
mod cmd;
/// Error and result types for `xtask`.
mod error;
/// Screenshot generation tasks.
mod screenshots;
/// Workspace discovery and metadata helpers.
mod workspace;

pub use error::{Error, Result};

/// Project helper commands (`cargo xtask ...`).
#[derive(Debug, Parser)]
#[command(author, version, about)]
struct Cli {
    /// The command to run.
    #[command(subcommand)]
    command: Xtask,
}

/// Subcommands for `cargo xtask`.
#[derive(Debug, Subcommand)]
enum Xtask {
    /// Build a release `.app` bundle for Hotki.
    Bundle(bundle::BundleArgs),
    /// Build a debug `.app` bundle for Hotki (dev icon + identifiers).
    BundleDev(bundle::BundleDevArgs),
    /// Generate UI screenshots for the README gallery.
    Screenshots,
}

/// Execute the `cargo xtask` CLI.
pub fn run() -> Result<()> {
    let cli = Cli::parse();
    let root_dir = workspace::workspace_root()?;

    match cli.command {
        Xtask::Bundle(args) => bundle::bundle_release(&root_dir, &args),
        Xtask::BundleDev(args) => bundle::bundle_dev(&root_dir, &args),
        Xtask::Screenshots => screenshots::screenshots(&root_dir),
    }
}
