//! rust - Rust-specific subcommands for managing offline Rust development environments.
//! Provides functionality for initializing projects, fetching modules,
//! and toggling between online/offline modes.

use clap_derive::Subcommand;

/// Cargo command enumeration for offline development operations.
#[derive(Subcommand, Debug)]
pub(crate) enum CargoCommands {
    //todo
}
