//! init - Scaffold a minimal Go project with go.mod and main.go.
//!
//! Provides the foundation for war go init: creates directory, writes go.mod,
//! and sets up a main.go with a blank import block ready for war go get.

use std::path::PathBuf;
use war_core::WarError;

// -------------------------------------------- Public API --------------------------------------------

/// Initialize a new Go project directory with go.mod and a minimal main.go.
///
/// Returns the path to the created project directory on success.
pub fn init_project(name: &str) -> Result<PathBuf, WarError> {
    // Phase 0 stub: return Ok with dummy path
    Ok(PathBuf::from(name))
}
