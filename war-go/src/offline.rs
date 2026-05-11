//! offline.rs - Generate shell export commands that route GOPROXY to the
//! local war cache at `~/.war/cache/go`, enabling fully offline Go builds.
//!
//! The primary entry-point is `generate_offline_exports()`, which prints
//! `export` statements suitable for `eval $(war go offline)`.  When these
//! variables are set, every Go module download resolves through the
//! `file://` protocol to the local cache — no network required (◕‿◕✿)
//!
//! `go_offline()` is the programmatic twin: it applies the same variables
//! to the *current process environment* and optionally persists them to the
//! user's shell profile.

use std::path::PathBuf;
use war_core::WarError;

// ----------------------- Public Functions -----------------------

/// Return the default war Go cache root: `~/.war/cache/go`.
///
/// This is the directory that `unpack_modules` populates and that
/// `generate_offline_exports` points `GOPROXY` at.
pub fn default_cache_root() -> Result<PathBuf, WarError> {
    let home = dirs::home_dir().ok_or_else(|| WarError::ShellDetectionError)?;
    Ok(home.join(".war").join("cache").join("go"))
}

/// Generate shell `export` statements that route Go to the local war cache.
///
/// The output is designed for `eval $(war go offline)`.  It sets:
///
/// - `GOPROXY=file:///<cache_root>` — forces Go to resolve modules from
///   the local file-system cache instead of the internet.
/// - `GONOSUMDB=*` — disables checksum database lookups (impossible offline).
/// - `GOSUMDB=off` — redundant safety-net to disable sumdb.
/// - `GOFLAGS=-mod=readonly` — prevents Go from silently downloading.
///
/// If the cache directory does not exist yet a warning is emitted, but the
/// exports are still generated so the caller can set them before unpacking.
pub fn generate_offline_exports() -> String {
    match default_cache_root() {
        Ok(cache_root) => {
            let cache_str = cache_root.display();
            let mut exports = Vec::new();

            // Construct the file:// URL.  Must be absolute for Go to accept it.
            exports.push(format!("export GOPROXY=file://{}",
                cache_str
            ));
            // Disable sum database — not reachable in air-gap.
            exports.push("export GONOSUMDB=*".to_string());
            exports.push("export GOSUMDB=off".to_string());
            // Prevent Go from auto-downloading anything.
            exports.push("export GOFLAGS=-mod=readonly".to_string());

            // Friendly comment so the user knows where these came from.
            let mut out = String::from("# war go offline — route Go to local cache\n");
            out.push_str(&exports.join("\n"));
            out.push('\n');

            if !cache_root.exists() {
                out.push_str(&format!(
                    "# ⚠  Cache dir {} does not exist yet. Run `war go unpack <archive>` first.\n",
                    cache_str
                ));
            }

            out
        }
        Err(_) => {
            "# ⚠  Could not determine home directory; cannot generate offline exports.\n".to_string()
        }
    }
}

/// Generate shell `unset` statements to restore online Go behaviour.
///
/// The output is designed for `eval $(war go online)`.
pub fn generate_online_exports() -> String {
    let mut out = String::from("# war go online — restore default Go behaviour\n");
    out.push_str("unset GOPROXY\n");
    out.push_str("unset GONOSUMDB\n");
    out.push_str("unset GOSUMDB\n");
    out.push_str("unset GOFLAGS\n");
    out
}

/// Apply offline environment variables to the current process.
///
/// Returns a list of `(variable, old_value)` pairs so the caller can
/// restore them later via `go_online()`.
///
/// `global` flag is reserved for future use: when true the variables will
/// also be persisted to the user's shell profile (`~/.bashrc`, etc.).
pub fn go_offline(
    _vendor_path: Option<PathBuf>,
    _global: bool,
) -> Result<Vec<(String, Option<String>)>, WarError> {
    let cache_root = default_cache_root()?;
    let cache_str = cache_root.display().to_string();

    let mut changes: Vec<(String, Option<String>)> = Vec::new();

    // Capture old values so we can restore them in go_online().
    for (key, value) in [
        ("GOPROXY", format!("file://{}", cache_str)),
        ("GONOSUMDB", "*".to_string()),
        ("GOSUMDB", "off".to_string()),
        ("GOFLAGS", "-mod=readonly".to_string()),
    ] {
        let old = std::env::var(key).ok();
        std::env::set_var(key, &value);
        changes.push((key.to_string(), old));
    }

    tracing::info!("✔ Offline mode enabled — GOPROXY=file://{}", cache_str);

    Ok(changes)
}

// ----------------------- Internal Helpers -----------------------

/// Restore previously captured environment variables (used by `go_online`).
#[allow(dead_code)]
fn restore_env(changes: &[(String, Option<String>)]) {
    for (key, old) in changes {
        match old {
            Some(val) => std::env::set_var(key, val),
            None => std::env::remove_var(key),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_offline_exports_contains_goproxy() {
        let exports = generate_offline_exports();
        assert!(exports.contains("GOPROXY=file://"), "Expected GOPROXY export, got:\n{}", exports);
        assert!(exports.contains("GONOSUMDB=*"), "Expected GONOSUMDB export");
        assert!(exports.contains("GOSUMDB=off"), "Expected GOSUMDB export");
        assert!(exports.contains("GOFLAGS=-mod=readonly"), "Expected GOFLAGS export");
    }

    #[test]
    fn test_generate_online_exports_unsets_vars() {
        let exports = generate_online_exports();
        assert!(exports.contains("unset GOPROXY"));
        assert!(exports.contains("unset GONOSUMDB"));
        assert!(exports.contains("unset GOSUMDB"));
        assert!(exports.contains("unset GOFLAGS"));
    }

    #[test]
    fn test_default_cache_root_under_war_dir() {
        let root = default_cache_root().expect("should resolve");
        assert!(root.ends_with(".war/cache/go"), "Unexpected root: {}", root.display());
    }

    #[test]
    fn test_go_offline_sets_env_vars() {
        let changes = go_offline(None, false).expect("should succeed");
        assert_eq!(changes.len(), 4, "Expected 4 env changes");

        // Verify the env was actually set.
        assert_eq!(
            std::env::var("GOPROXY").unwrap(),
            format!("file://{}", default_cache_root().unwrap().display())
        );

        // Clean up so other tests aren't affected.
        for (key, old) in &changes {
            match old {
                Some(val) => std::env::set_var(key, val),
                None => std::env::remove_var(key),
            }
        }
    }
}
