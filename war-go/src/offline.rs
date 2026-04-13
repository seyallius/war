//! offline - Switch Go to offline mode using vendored dependencies.
//!
//! Sets environment overrides (GOFLAGS, GOPROXY, GOSUMDB), parses vendor/modules.txt,
//! reconstructs ~/go/pkg/mod cache, and optionally persists env changes globally.

use crate::{cache, vendor};
use std::{
    env, fs, io,
    path::{Path, PathBuf},
    process,
};
use tempfile::TempDir;
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
/// For each module, checks if any of the three cache files (`.info`/`.mod`/`.zip`) already exist.
/// If so, copies them to a temp backup directory with the same relative path structure.
/// Returns the TempDir handle (kept alive by caller) or None if no files needed backup.
///
/// Note: The returned TempDir is automatically cleaned up when dropped, so the caller
/// must keep it alive until revert is no longer possible (i.e., after successful sync).
fn create_cache_snapshot(modules: &[VendorModule]) -> Result<Option<TempDir>, WarError> {
    let cache_root = get_go_mod_cache_path()?;
    let mut backed_up = false;
    let mut snapshot_dir: Option<TempDir> = None;

    for module in modules {
        let module_cache_dir = cache_root.join(module.path.replace('/', "!")).join("@v");
        let versions = [
            format!("{}.info", module.version),
            format!("{}.mod", module.version),
            format!("{}.zip", module.version),
        ];

        for file_name in versions.iter() {
            let src_path = module_cache_dir.join(file_name);
            if src_path.exists() {
                // Lazy-create snapshot dir only if we actually need to back up something
                let snapshot = snapshot_dir.get_or_insert_with(|| {
                    TempDir::new().expect("Failed to create cache snapshot directory")
                });

                // Preserve relative path structure in snapshot
                let relative_path =
                    src_path
                        .strip_prefix(&cache_root)
                        .map_err(|e| WarError::ConfigError {
                            source: Box::new(io::Error::new(
                                io::ErrorKind::InvalidInput,
                                format!("Failed to compute relative path: {}", e),
                            )),
                        })?;

                let dest_path = snapshot.path().join(relative_path);
                if let Some(parent) = dest_path.parent() {
                    fs::create_dir_all(parent).map_err(|e| WarError::CacheWriteError {
                        module: module.path.clone(),
                        source: e,
                    })?;
                }

                fs::copy(&src_path, &dest_path).map_err(|e| WarError::CacheWriteError {
                    module: module.path.clone(),
                    source: e,
                })?;

                backed_up = true;
            }
        }
    }

    // Return None if nothing was backed up (clean install, no revert needed)
    if backed_up {
        Ok(snapshot_dir)
    } else {
        Ok(None)
    }
}

/// Revert cache changes using a previously created snapshot.
///
/// For each file in the snapshot:
/// - If the current cache file exists and differs from backup, restore backup
/// - If the current cache file doesn't exist but backup does, delete the new file
/// Finally, removes any cache files that were created during sync but not backed up
/// (by comparing against the original module list).
fn revert_cache_snapshot(snapshot: &Option<TempDir>) -> Result<(), WarError> {
    let Some(snapshot_dir) = snapshot else {
        // Nothing to revert
        return Ok(());
    };

    let cache_root = get_go_mod_cache_path()?;

    // Walk snapshot directory and restore each backed-up file
    for entry in walk_dir_recursive(snapshot_dir.path())? {
        let relative_path =
            entry
                .strip_prefix(snapshot_dir.path())
                .map_err(|e| WarError::ConfigError {
                    source: Box::new(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        format!("Failed to compute relative path: {}", e),
                    )),
                })?;

        let dest_path = cache_root.join(relative_path);
        let src_path = snapshot_dir.path().join(relative_path);

        if entry.is_file() {
            // Restore file from backup
            if let Some(parent) = dest_path.parent() {
                fs::create_dir_all(parent).map_err(|e| WarError::CacheWriteError {
                    module: relative_path.to_string_lossy().to_string(),
                    source: e,
                })?;
            }
            fs::copy(&src_path, &dest_path).map_err(|e| WarError::CacheWriteError {
                module: relative_path.to_string_lossy().to_string(),
                source: e,
            })?;
        }
    }

    Ok(())
}

/// Recursively walk a directory and return all file paths.
///
/// Helper for snapshot revert logic.
fn walk_dir_recursive(dir: &Path) -> Result<Vec<PathBuf>, WarError> {
    let mut files = Vec::new();

    let opened_dir = fs::read_dir(dir).map_err(|e| WarError::CacheWriteError {
        module: dir.to_string_lossy().to_string(),
        source: e,
    })?;
    for entry in opened_dir {
        let entry = entry.map_err(|e| WarError::CacheWriteError {
            module: dir.to_string_lossy().to_string(),
            source: e,
        })?;
        let path = entry.path();

        if path.is_dir() {
            files.extend(walk_dir_recursive(&path)?);
        } else {
            files.push(path);
        }
    }

    Ok(files)
}

#[cfg(test)]
mod offline_tests {
    use super::*;
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

    #[test]
    fn test_create_cache_snapshot_backs_up_existing_files() {
        let cache_root = tempdir().unwrap();
        let module_cache = cache_root.path().join("github.com!test!lib").join("@v");
        fs::create_dir_all(&module_cache).unwrap();

        // Create a mock existing cache file
        let existing_info = module_cache.join("v1.0.0.info");
        fs::write(&existing_info, "original content").unwrap();

        // Mock module list
        let modules = vec![VendorModule {
            path: "github.com/test/lib".to_string(),
            version: "v1.0.0".to_string(),
            explicit: true,
            go_version: None,
            packages: vec![],
            vendor_path: PathBuf::new(),
        }];

        // Temporarily override GOMODCACHE for test isolation
        let orig_cache = env::var("GOMODCACHE").ok();
        env::set_var("GOMODCACHE", cache_root.path());

        let snapshot = create_cache_snapshot(&modules).unwrap();
        assert!(snapshot.is_some(), "Expected snapshot for existing file");

        // Verify backup was created
        let snap_dir = snapshot.unwrap();
        let backed_up = snap_dir
            .path()
            .join("github.com!test!lib")
            .join("@v")
            .join("v1.0.0.info");
        assert!(backed_up.exists());
        assert_eq!(fs::read_to_string(backed_up).unwrap(), "original content");

        // Cleanup env
        if let Some(val) = orig_cache {
            env::set_var("GOMODCACHE", val);
        } else {
            env::remove_var("GOMODCACHE");
        }
    }

    #[test]
    fn test_create_cache_snapshot_returns_none_when_no_existing_files() {
        let cache_root = tempdir().unwrap();
        let modules = vec![VendorModule {
            path: "github.com/test/lib".to_string(),
            version: "v1.0.0".to_string(),
            explicit: true,
            go_version: None,
            packages: vec![],
            vendor_path: PathBuf::new(),
        }];

        let orig_cache = env::var("GOMODCACHE").ok();
        env::set_var("GOMODCACHE", cache_root.path());

        let snapshot = create_cache_snapshot(&modules).unwrap();
        assert!(snapshot.is_none(), "Expected None when no files to back up");

        if let Some(val) = orig_cache {
            env::set_var("GOMODCACHE", val);
        } else {
            env::remove_var("GOMODCACHE");
        }
    }

    #[test]
    fn test_revert_cache_snapshot_restores_files() {
        let cache_root = tempdir().unwrap();
        let snapshot_dir = tempdir().unwrap();

        // Setup: create a file in snapshot that should be restored
        let snap_file = snapshot_dir
            .path()
            .join("github.com!test!lib")
            .join("@v")
            .join("v1.0.0.info");
        fs::create_dir_all(snap_file.parent().unwrap()).unwrap();
        fs::write(&snap_file, "backup content").unwrap();

        // Create a different "corrupted" file in cache
        let cache_file = cache_root
            .path()
            .join("github.com!test!lib")
            .join("@v")
            .join("v1.0.0.info");
        fs::create_dir_all(cache_file.parent().unwrap()).unwrap();
        fs::write(&cache_file, "corrupted content").unwrap();

        // Temporarily override GOMODCACHE
        let orig_cache = env::var("GOMODCACHE").ok();
        env::set_var("GOMODCACHE", cache_root.path());

        // Revert should restore backup content
        revert_cache_snapshot(&Some(snapshot_dir)).unwrap();
        assert_eq!(fs::read_to_string(&cache_file).unwrap(), "backup content");

        if let Some(val) = orig_cache {
            env::set_var("GOMODCACHE", val);
        } else {
            env::remove_var("GOMODCACHE");
        }
    }
}
