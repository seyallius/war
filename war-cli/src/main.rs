//! main - Entry point for the war CLI binary.
//!
//! Parses command-line arguments via clap and dispatches to domain logic
//! from war-core and war-go.

use crate::{cli::Cli, dispatch::dispatch};
use clap::Parser;
use std::{env, process};

mod cli;
mod commands;
mod dispatch;

/// Parse CLI args, initialize tracing, dispatch to the appropriate command handler.
#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    // Initialize tracing subscriber based on verbosity flag
    let subscriber = tracing_subscriber::fmt();
    if cli.verbose {
        subscriber.with_max_level(tracing::Level::DEBUG).init();
    } else {
        subscriber.with_max_level(tracing::Level::INFO).init();
    }

    tracing::debug!("CLI parsed: {:?}", cli);

    let exit_code = match &cli.command {
        None => {
            tracing::warn!("war v{}", env!("CARGO_PKG_VERSION"));
            tracing::warn!("Use --help for usage information.");
            0
        }
        Some(command) => dispatch(command).await,
    };

    process::exit(exit_code);
}
