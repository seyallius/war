//! online.rs - Restore Go to standard online module resolution.
//!
//! Unsets or restores environment variables overridden by `go_offline`,
//! and cleans up any session/global configuration backups.
//!
//! When the CLI runs `war go online`, it prints the `generate_online_exports()`
//! output to stdout so the user can `eval $(war go online)` to unset the
//! variables in their shell session.

use war_core::WarError;

// ----------------------- Public Functions -----------------------

/// Restore Go's default online behavior by cleaning up env overrides.
///
/// `global` flag is reserved for future use: when true, revert persistent
/// shell profile changes made by `go_offline(global=true)`.
pub fn go_online(_global: bool) -> Result<(), WarError> {
    // Unset the offline env vars from the current process.
    for key in &["GOPROXY", "GONOSUMDB", "GOSUMDB", "GOFLAGS"] {
        std::env::remove_var(key);
    }

    tracing::info!("✔ Online mode restored — GOPROXY/GOSUMDB/GONOSUMDB/GOFLAGS unset");
    Ok(())
}
