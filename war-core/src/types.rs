//! types.rs - Shared domain types used across war-cli, war-go, and future crates.
//!
//! Defines the core data structures that flow between the CLI layer, domain
//! logic crates, and the persistent configuration file (`~/.war/war.lock`).
//! Every type here is serialisable so it can round-trip through TOML.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Represents a single vendored module parsed from vendor/modules.txt.
#[derive(Debug, Clone, PartialEq)]
pub struct VendorModule {
    /// Module import path (e.g. "github.com/gin-gonic/gin").
    pub path: String,
    /// Module version (e.g. "v1.9.1").
    pub version: String,
    /// Whether this module is marked `## explicit`.
    pub explicit: bool,
    /// Minimum Go version requirement (from `## explicit; go 1.20`).
    pub go_version: Option<String>,
    /// List of packages (subdirectories) used from this module.
    pub packages: Vec<String>,
    /// Filesystem path to the vendored source directory.
    pub vendor_path: PathBuf,
}

/// Represents a single Go module entry parsed from vendor/modules.txt.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModuleInfo {
    /// Module path (e.g., "github.com/gin-gonic/gin").
    pub path: String,
    /// Version string (e.g., "v1.9.1").
    pub version: String,
    /// Optional hash from modules.txt for integrity verification.
    pub hash: Option<String>,
    /// Path to the vendored source directory.
    pub vendor_path: PathBuf,
}

/// Result of a module sync operation during cache reconstruction.
#[derive(Debug, Clone)]
pub struct SyncResult {
    /// Module that was processed.
    pub module: ModuleInfo,
    /// Whether the sync succeeded.
    pub success: bool,
    /// Paths to artifacts created (for cleanup on revert).
    pub artifacts: Vec<PathBuf>,
    /// Optional error message if failed.
    pub error: Option<String>,
}

/// Represents the outcome of an offline/online toggle operation.
#[derive(Debug, Clone)]
pub struct ToggleResult {
    /// Whether the operation succeeded.
    pub success: bool,
    /// Modules that were successfully synced.
    pub synced: Vec<ModuleInfo>,
    /// Modules that failed (with reasons).
    pub failed: Vec<(ModuleInfo, String)>,
    /// Environment variables that were modified.
    pub env_changes: Vec<(String, Option<String>)>,
}

/// A single staged module entry — a `(module_path, version)` pair.
///
/// Stored in `GoConfig::staged_modules` inside `~/.war/war.lock`.  The
/// module path uses `/` separators (e.g. `github.com/gin-gonic/gin`) so
/// it matches the archive path format produced by `pack_modules`.
/// Deduplication is by `(module, version)` tuple — adding the same pair
/// twice is a no-op.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct StagedModule {
    /// Go module path, e.g. `github.com/gin-gonic/gin`.
    pub module: String,
    /// Semantic version, e.g. `v1.9.1`.
    pub version: String,
}

/// Go-specific configuration tracked in war.lock.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoConfig {
    /// Path to the last-used vendor directory.
    pub last_vendor_path: Option<PathBuf>,
    /// Timestamp of the last successful sync operation.
    pub last_sync_timestamp: Option<DateTime<Utc>>,
    /// Go version used during last sync (for compatibility checks).
    pub go_version: Option<String>,
    /// Modules explicitly staged for `pack --staged` / `unpack --staged`.
    /// Populated by `war go get` (auto-stage) and `war go stage add`.
    /// Persisted to `~/.war/war.lock` on every mutation.
    #[serde(default)]
    pub staged_modules: Vec<StagedModule>,
}

impl Default for GoConfig {
    fn default() -> Self {
        Self {
            last_vendor_path: None,
            last_sync_timestamp: None,
            go_version: None,
            staged_modules: Vec::new(),
        }
    }
}

/// Rust-specific configuration (placeholder for future war-rust crate).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RustConfig {
    /// Path to the last-used Cargo vendor directory.
    pub last_vendor_path: Option<PathBuf>,
    /// Timestamp of last sync.
    pub last_sync_timestamp: Option<DateTime<Utc>>,
}
