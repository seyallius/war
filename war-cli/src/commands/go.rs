//! go - Go-specific subcommands for managing offline Go development environments.
//! Provides functionality for initializing projects, fetching modules,
//! and toggling between online/offline modes.

use clap_derive::Subcommand;

/// Go command enumeration for offline development operations.
#[derive(Subcommand, Debug)]
pub(crate) enum GoCommands {
    /// Scaffold a minimal Go project with go.mod and main.go
    Init {
        /// Name of the project directory to create
        #[arg(short, long, default_value = "war-project")]
        name: String,
    },
    /// Fetch a module, auto-import it, and vendor dependencies
    Get {
        /// Module path (e.g., github.com/gin-gonic/gin[@v1.9.1])
        #[arg(value_name = "MODULE")]
        module: String,
    },
    /// Switch to offline mode using vendored dependencies
    Offline {
        /// Path to vendor directory (defaults: war.lock → ./vendor)
        #[arg(short, long)]
        vendor: Option<String>,
        /// Persist environment changes to shell profile
        #[arg(short, long)]
        global: bool,
    },
    /// Restore online mode and default Go behavior
    Online {
        /// Revert global shell profile changes
        #[arg(short, long)]
        global: bool,
    },
    /// Verify offline mode is working (dry-run build check)
    Verify,
}
