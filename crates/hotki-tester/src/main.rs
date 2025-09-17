#![warn(missing_docs)]

//! Entry point for the `hotki-tester` binary.

mod backend;
mod cli;
mod diagnostics;
mod error;
mod place;

use std::process;

use clap::Parser;
use tracing::error;
use tracing_subscriber::{fmt, prelude::*, registry};

use crate::{
    cli::{Cli, Commands},
    error::Result,
};

fn main() {
    if let Err(err) = run() {
        error!("{err}");
        eprintln!("error: {err}");
        process::exit(1);
    }
}

/// Parse CLI arguments, install logging, and dispatch to the chosen subcommand.
fn run() -> Result<()> {
    let Cli { log, command } = Cli::parse();
    let log_spec = logging::compute_spec(
        log.trace,
        log.debug,
        log.log_level.as_deref(),
        log.log_filter.as_deref(),
    );
    let env_filter = logging::env_filter_from_spec(&log_spec);
    registry()
        .with(env_filter)
        .with(fmt::layer().without_time())
        .try_init()
        .ok();

    match command {
        Commands::Place(args) => place::run(&args, &log_spec),
    }
}
