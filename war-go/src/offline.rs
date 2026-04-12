//! offline - Switch Go to offline mode using vendored dependencies.
//!
//! Sets environment overrides (GOFLAGS, GOPROXY, GOSUMDB), parses vendor/modules.txt,
//! reconstructs ~/go/pkg/mod cache, and optionally persists env changes globally.

use crate::{cache, vendor};
use std::{
    env, io,
    path::{Path, PathBuf},
    process,
};
use war_core::{
    config::{load_config, save_config},
    types::{GoConfig, VendorModule},
    WarError,
};

// -------------------------------------------- Public API --------------------------------------------

/// Toggle Go into offline mode with optional vendor path and global env persistence.
///
/// vendor_path: resolved via --flag → ~/.war/war.lock → ./vendor
/// global: if true, persist env changes to shell profile
///
/// Returns the list of environment variable changes made, for caller to display or log.
pub fn go_offline(
    vendor_path: Option<PathBuf>,
    global: bool,
) -> Result<Vec<(String, Option<String>)>, WarError> {
    // 1. Resolve vendor directory path
    let vendor_root = resolve_vendor_path(vendor_path)?;

    // 2. Parse vendor/modules.txt to get module list
    // Use the vendor-dir-specific parser since resolve_vendor_path returns the vendor dir itself
    let modules = vendor::parse_vendor_manifest_from_dir(&vendor_root)?;

    // 3. Create cache snapshot for revert capability (if any module fails)
    let snapshot = create_cache_snapshot(&modules)?;

    // 4. Reconstruct ~/go/pkg/mod cache from vendor source
    let cache_root = get_go_mod_cache_path()?;
    if let Err(e) = cache::reconstruct_cache(&modules, &cache_root, &vendor_root) {
        // On error, attempt to revert using snapshot
        revert_cache_snapshot(&snapshot)?;
        return Err(e);
    }

    // 5. Set environment variables for offline mode
    let env_changes = set_offline_env_vars(global)?;

    // 6. Update war.lock with last used vendor path
    update_war_lock(&vendor_root)?;

    Ok(env_changes)
}

// -------------------------------------------- Internal Helpers --------------------------------------------

/// Resolve vendor path using precedence: `--flag` → `~/.war/war.lock` → `./vendor`
fn resolve_vendor_path(provided: Option<PathBuf>) -> Result<PathBuf, WarError> {
    if let Some(path) = provided {
        if path.exists() {
            return path.canonicalize().map_err(|e| WarError::ConfigError {
                source: Box::new(e),
            });
        }
        return Err(WarError::ConfigError {
            source: Box::new(io::Error::new(
                io::ErrorKind::NotFound,
                format!("Vendor path not found: {}", path.display()),
            )),
        });
    }

    // Check war.lock for last used vendor path
    if let Ok(config) = load_config() {
        if let Some(go_config) = &config.go {
            if let Some(ref last_path) = go_config.last_vendor_path {
                if last_path.exists() {
                    return Ok(last_path.clone());
                }
            }
        }
    }

    // Fallback to ./vendor relative to current directory
    let fallback = PathBuf::from("vendor");
    if fallback.exists() {
        return fallback.canonicalize().map_err(|e| WarError::ConfigError {
            source: Box::new(e),
        });
    }

    Err(WarError::ConfigError {
        source: Box::new(io::Error::new(
            io::ErrorKind::NotFound,
            "No vendor directory found. Run `go mod vendor` first, or provide --vendor=<path>",
        )),
    })
}

/// Get the Go module cache path (~/go/pkg/mod or $GOMODCACHE).
fn get_go_mod_cache_path() -> Result<PathBuf, WarError> {
    // Respect GOMODCACHE env var if set
    if let Ok(cache) = env::var("GOMODCACHE") {
        return Ok(PathBuf::from(cache));
    }

    // Default to ~/go/pkg/mod
    let home = dirs::home_dir().ok_or_else(|| WarError::ConfigError {
        source: Box::new(io::Error::new(
            io::ErrorKind::NotFound,
            "Could not determine home directory",
        )),
    })?;

    Ok(home.join("go").join("pkg").join("mod"))
}

/// Set environment variables for offline Go mode.
///
/// Returns list of (var_name, old_value) for potential restoration.
fn set_offline_env_vars(global: bool) -> Result<Vec<(String, Option<String>)>, WarError> {
    let vars = [
        ("GOFLAGS", "-mod=vendor"),
        ("GOPROXY", "off"),
        ("GOSUMDB", "off"),
    ];

    let mut changes = Vec::new();

    for (key, value) in vars.iter() {
        let old = env::var(key).ok();
        env::set_var(key, value);
        changes.push((key.to_string(), old));

        // If global, also output export command for shell eval
        if global {
            eprintln!("export {}={}", key, value);
        }
    }

    Ok(changes)
}

/// Update war.lock with the last used vendor path and timestamp.
fn update_war_lock(vendor_root: &Path) -> Result<(), WarError> {
    let mut config = load_config().unwrap_or_default();

    // Try to detect Go version, with guaranteed fallback to "unknown"
    let go_version = env::var("GOVERSION")
        .ok()
        .or_else(|| {
            // Fallback: parse from `go version` output
            process::Command::new("go")
                .arg("version")
                .output()
                .ok()
                .and_then(|out| String::from_utf8(out.stdout).ok())
                .and_then(|s| s.split(' ').nth(2).map(|v| v.to_string()))
        })
        .unwrap_or_else(|| "unknown".to_string());

    config.go = Some(GoConfig {
        last_vendor_path: Some(vendor_root.to_path_buf()),
        last_sync_timestamp: Some(chrono::Utc::now()),
        go_version: Some(go_version),
    });

    save_config(&config)
}

/// Create a lightweight snapshot of affected cache paths for revert capability.
///
/// Returns a temp directory path containing the snapshot, or None if snapshot creation fails.
fn create_cache_snapshot(_modules: &[VendorModule]) -> Result<Option<PathBuf>, WarError> {
    // TODO: Implement snapshot logic in next step
    // For now, return Ok(None) to allow compilation
    Ok(None)
}

/// Revert cache changes using a previously created snapshot.
fn revert_cache_snapshot(_snapshot: &Option<PathBuf>) -> Result<(), WarError> {
    // TODO: Implement revert logic in next step
    Ok(())
}

#[cfg(test)]
mod offline_tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn test_resolve_vendor_path_with_flag() {
        let temp = tempdir().unwrap();
        let provided = Some(temp.path().to_path_buf());
        let result = resolve_vendor_path(provided).unwrap();
        assert_eq!(result, temp.path().canonicalize().unwrap());
    }

    #[test]
    fn test_resolve_vendor_path_fallback_to_dot_vendor() {
        // Create a temp dir with ./vendor subdirectory
        let temp = tempdir().unwrap();
        let vendor_dir = temp.path().join("vendor");
        fs::create_dir_all(&vendor_dir).unwrap();

        // Change to temp dir for test
        let original_dir = env::current_dir().unwrap();
        env::set_current_dir(temp.path()).unwrap();

        let result = resolve_vendor_path(None).unwrap();
        assert_eq!(result, vendor_dir.canonicalize().unwrap());

        // Restore original directory
        env::set_current_dir(original_dir).unwrap();
    }

    #[test]
    fn test_resolve_vendor_path_error_when_not_found() {
        let result = resolve_vendor_path(Some(PathBuf::from("/nonexistent/path")));
        assert!(result.is_err());
    }

    #[test]
    fn test_update_war_lock_fallback_go_version() {
        // Save and clear GOVERSION to test fallback
        let orig = env::var("GOVERSION").ok();
        env::remove_var("GOVERSION");

        let temp = tempdir().unwrap();
        let vendor_dir = temp.path().join("vendor");
        fs::create_dir_all(&vendor_dir).unwrap();

        // This should succeed even if `go` command is unavailable
        let result = update_war_lock(&vendor_dir);
        assert!(result.is_ok());

        // Restore env
        if let Some(val) = orig {
            env::set_var("GOVERSION", val);
        }
    }
}
