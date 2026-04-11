//! verify - Dry-run check to confirm offline mode is working.
//!
//! Runs `go list -m all -mod=vendor` and `go build -x` to detect
//! any unexpected network fallback attempts.

use war_core::WarError;

// -------------------------------------------- Public API --------------------------------------------

/// Verify that Go is correctly configured for offline development.
///
/// Returns Ok(()) if no network calls are detected; Err otherwise.
pub fn verify_offline() -> Result<(), WarError> {
    // Phase 0 stub
    Ok(())
}
