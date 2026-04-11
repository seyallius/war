//! main - Entry point for the war CLI binary.
//!
//! Parses command-line arguments via clap and dispatches to domain logic
//! from war-core and war-go. Currently stubbed for Phase 0 bootstrap.

#![warn(missing_docs)]

use crate::cli::{Cli, Commands};
use clap::Parser;

mod cli;
mod commands;

/// Entry point: parse args, initialize tracing, dispatch to command handler.
fn main() {
    let cli = Cli::parse();

    // Initialize tracing subscriber (verbose mode)
    if cli.verbose {
        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::DEBUG)
            .init();
    } else {
        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::INFO)
            .init();
    }

    tracing::debug!("CLI parsed: {:?}", cli);

    // Phase 0 stub: just print confirmation
    match &cli.command {
        None => {
            println!("war v{}", env!("CARGO_PKG_VERSION"));
            println!("Use --help for usage information");
        }
        Some(command) => match command {
            Commands::Rust { .. } => {
                unimplemented!("implement this command")
            }
            Commands::Go { subcommand } => {
                println!("🔧 Go subcommand: {:?}", subcommand);
                println!("(Implementation coming in Phase 1)");
            }
        },
    }
}
