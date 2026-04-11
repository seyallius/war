//! vendor - Parse vendor/modules.txt and extract module metadata.
//!
//! Provides a pure function to parse Go's vendor manifest into
//! a structured Vec<ModuleInfo> for cache reconstruction.

use std::path::PathBuf;
use war_core::{types::ModuleInfo, WarError};

// -------------------------------------------- Public API --------------------------------------------

/// Parse vendor/modules.txt into a vector of ModuleInfo entries.
///
/// vendor_root: path to the vendor directory containing modules.txt
pub fn parse_vendor_manifest(_vendor_root: &PathBuf) -> Result<Vec<ModuleInfo>, WarError> {
    // Phase 0 stub: return empty vec
    Ok(Vec::new())
}
