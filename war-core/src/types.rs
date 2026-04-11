//! types - Shared domain types used across war-cli, war-go, and future crates.
//!
//! These types represent the core data structures for module synchronization,
//! cache reconstruction, and operation results.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

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

/// Go-specific configuration tracked in war.lock.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoConfig {
    /// Path to the last-used vendor directory.
    pub last_vendor_path: Option<PathBuf>,
    /// Timestamp of the last successful sync operation.
    pub last_sync_timestamp: Option<DateTime<Utc>>,
    /// Go version used during last sync (for compatibility checks).
    pub go_version: Option<String>,
}

/// Rust-specific configuration (placeholder for future war-rust crate).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RustConfig {
    /// Path to the last-used Cargo vendor directory.
    pub last_vendor_path: Option<PathBuf>,
    /// Timestamp of last sync.
    pub last_sync_timestamp: Option<DateTime<Utc>>,
}
