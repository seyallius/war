//! cli - CLI module defining command-line interface structure and argument parsing
//! for the war offline development toolkit.

use crate::commands::{GoCommands, RustCommands};
use clap_derive::{Parser, Subcommand};

/// war — Offline development toolkit for Rust & Go (and future languages).
///
/// Toggle between online and offline modes, manage vendored dependencies,
/// and reconstruct module caches for air-gapped development.
#[derive(Parser, Debug)]
#[command(
    name = "war",
    version,
    about = "Offline development toolkit for Rust & Go (and future languages)",
    long_about = None
)]
pub(crate) struct Cli {
    /// Enable verbose output for debugging
    #[arg(short, long, global = true)]
    pub verbose: bool,

    /// Subcommands for language-specific operations
    #[command(subcommand)]
    pub command: Option<Commands>,
}

/// Top-level command enumeration for language-specific operations.
#[derive(Subcommand, Debug)]
pub(crate) enum Commands {
    /// Rust-specific operations (init, add, offline, online, check)
    Rust {
        #[command(subcommand)]
        subcommand: RustCommands,
    }, // Future: uncomment when war-rust is ready
    /// Go-specific operations (init, get, offline, online, verify)
    Go {
        #[command(subcommand)]
        subcommand: GoCommands,
    },
}
