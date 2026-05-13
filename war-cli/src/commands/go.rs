//! go.rs - Go-specific subcommands for managing offline Go development environments.
//!
//! Provides the full set of CLI verbs: `init`, `get`, `pack`, `unpack`,
//! `stage`, `offline`, `online`, `sync`, and `verify`.  Each variant carries only
//! the flags it needs — the heavy lifting is delegated to `war_go`.

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

    /// Fetch a module, auto-import it, vendor dependencies, and auto-stage it
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

        /// Only pack modules that are in the staged list (~/.war/war.lock)
        #[arg(long)]
        staged: bool,
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

        /// Only extract modules that are in the staged list (~/.war/war.lock)
        #[arg(long)]
        staged: bool,
    },

    /// Manage the staged-module cart for selective pack/unpack
    Stage {
        #[command(subcommand)]
        subcommand: StageCommands,
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

    /// Restore online mode and default Go behaviour
    Online {
        /// Revert global shell profile changes
        #[arg(short, long)]
        global: bool,
    },

    /// Copy ~/.war/cache/go → $GOMODCACHE so `go build` works without `eval $(war go offline)`.
    ///
    /// Permanently hydrates the native Go module cache from the war cache.
    /// Files already present with matching content are skipped (idempotent).
    /// Existing files with different content are reported as collisions and
    /// left untouched — war never silently overwrites user-modified files.
    Sync {
        /// Source war cache directory (default: ~/.war/cache/go)
        #[arg(short, long)]
        cache: Option<String>,

        /// Target native Go module cache directory.
        /// Defaults to $GOMODCACHE, then $GOPATH/pkg/mod, then ~/go/pkg/mod.
        #[arg(short, long)]
        dest: Option<String>,
    },

    /// Verify offline mode is working (dry-run build check)
    Verify,
}

/// Subcommands for the `war go stage` verb.
#[derive(Subcommand, Debug)]
pub(crate) enum StageCommands {
    /// List all staged modules
    List,

    /// Add a module to the staged list
    Add {
        /// Module path (e.g., github.com/gin-gonic/gin)
        #[arg(value_name = "MODULE")]
        module: String,

        /// Version (e.g., v1.9.1)
        #[arg(value_name = "VERSION")]
        version: String,
    },

    /// Remove a module from the staged list
    Remove {
        /// Module path (e.g., github.com/gin-gonic/gin)
        #[arg(value_name = "MODULE")]
        module: String,

        /// Version (e.g., v1.9.1)
        #[arg(value_name = "VERSION")]
        version: String,
    },

    /// Clear all staged modules
    Clear,
}
