//! go.rs - Go-specific subcommands for managing offline Go development environments.
//!
//! Provides the full set of CLI verbs: `init`, `get`, `pack`, `unpack`,
//! `offline`, `online`, and `verify`.  Each variant carries only the flags
//! it needs — the heavy lifting is delegated to `war_go`.

use clap_derive::Subcommand;

/// Go command enumeration for offline development operations.
#[derive(Subcommand, Debug)]
pub(crate) enum GoCommands {
    /// Scaffold a minimal Go project with go.mod and main.go
    Init {
        /// Name of the project directory to create
        #[arg(default_value = "war-project")]
        name: String,
    },

    /// Fetch a module, auto-import it, and vendor dependencies
    Get {
        /// Module path (e.g., github.com/gin-gonic/gin[@v1.9.1])
        #[arg(value_name = "MODULE")]
        module: String,
    },

    /// Pack Go module cache into a portable zip archive
    Pack {
        /// Path to the Go cache root to pack (default: ~/.war/cache/go)
        #[arg(short, long)]
        cache: Option<String>,

        /// Output zip file path (default: war-pack.zip in current directory)
        #[arg(short, long, default_value = "war-pack.zip")]
        output: String,
    },

    /// Unpack a war archive into the local Go module cache
    Unpack {
        /// Path to the zip archive to extract
        #[arg(value_name = "ARCHIVE")]
        archive: String,

        /// Target cache directory (default: ~/.war/cache/go)
        #[arg(short, long)]
        cache: Option<String>,

        /// List files that would be extracted without writing them
        #[arg(long)]
        dry_run: bool,
    },

    /// Switch to offline mode using the local war cache
    Offline {
        /// Path to vendor directory (defaults: war.lock → ./vendor)
        #[arg(long)]
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
