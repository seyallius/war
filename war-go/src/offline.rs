//! offline - Switch Go to offline mode using vendored dependencies.
//!
//! Sets environment overrides (GOFLAGS, GOPROXY, GOSUMDB), parses vendor/modules.txt,
//! reconstructs ~/go/pkg/mod cache, and optionally persists env changes globally.

use std::path::PathBuf;
use war_core::WarError;

// -------------------------------------------- Public API --------------------------------------------

/// Toggle Go into offline mode with optional vendor path and global env persistence.
///
/// vendor_path: resolved via --flag → war.lock → ./vendor
/// global: if true, persist env changes to shell profile
pub fn go_offline(_vendor_path: Option<PathBuf>, _global: bool) -> Result<(), WarError> {
    // Phase 0 stub
    Ok(())
}
