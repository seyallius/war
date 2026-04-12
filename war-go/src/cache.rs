//! cache - Reconstruct ~/go/pkg/mod cache from vendored modules.
//!
//! Handles the zero-network sync: generating .info, .mod, and .zip
//! files per module version, with parallel processing and atomic writes.

use chrono::{DateTime, Utc};
use rayon::prelude::*;
use serde::Serialize;
use std::{
    fs,
    path::{Path, PathBuf},
    sync::Mutex,
};
use tempfile::NamedTempFile;
use war_core::{types::VendorModule, WarError};

// -------------------------------------------- Public API --------------------------------------------

/// Reconstruct Go module cache entries from vendored source.
///
/// - modules: list of VendorModule from vendor/modules.txt parsing
/// - cache_root: target directory (usually `~/.go/pkg/mod/cache/download`)
///
/// Uses rayon for parallel processing: each module's `.info`/`.mod`/`.zip` generation
/// runs concurrently, significantly speeding up cache reconstruction for large
/// vendor directories (200+ modules).
pub fn reconstruct_cache(modules: &[VendorModule], cache_root: &Path) -> Result<(), WarError> {
    // Collect errors from parallel iteration in a thread-safe way
    let errors = Mutex::new(Vec::<(String, WarError)>::new());

    modules.par_iter().try_for_each(|module| {
        match sync_module_cache(module, cache_root) {
            Ok(()) => Ok::<(), WarError>(()),
            Err(e) => {
                errors.lock().unwrap().push((module.path.clone(), e));
                // Continue processing other modules; caller can decide to abort/retry
                Ok(())
            }
        }
    })?;

    // If any errors occurred, report the first one (interactive handler can use full list)
    let mut err_vec = errors.into_inner().unwrap();
    if !err_vec.is_empty() {
        let (module, err) = err_vec.remove(0);
        return Err(WarError::ModuleSyncError {
            module,
            reason: err.to_string(),
            partial_artifacts: vec![], // TODO: track partial artifacts during sync
            recoverable: true,         // Most cache write errors are retryable
        });
    }

    Ok(())
}

// -------------------------------------------- Internal Helpers --------------------------------------------

/// Sync a single module's cache entries (`.info`, `.mod`, `.zip`).
///
/// This function is called in parallel by rayon, so it must not mutate shared state.
fn sync_module_cache(module: &VendorModule, cache_root: &Path) -> Result<(), WarError> {
    // Go cache layout: <cache_root>/<module_path_with_!>/@v/
    let module_cache_dir = cache_root.join(module.path.replace('/', "!")).join("@v");
    fs::create_dir_all(&module_cache_dir).map_err(|e| WarError::CacheWriteError {
        module: module.path.clone(),
        source: e,
    })?;

    // Generate and write .info file (atomic)
    write_info_file(module, &module_cache_dir)?;

    // Copy .mod file from vendor source (atomic)
    write_mod_file(module, &module_cache_dir)?;

    // .mod and .zip will be implemented in next steps
    Ok(())
}

/// Write the .info file atomically using a temp file + rename.
fn write_info_file(module: &VendorModule, cache_dir: &PathBuf) -> Result<(), WarError> {
    let info_path = cache_dir.join(format!("{}.info", module.version));
    let content = generate_info_content(module)?;

    // Atomic write: write to temp file in same directory, then rename
    let temp_file = NamedTempFile::new_in(cache_dir).map_err(|e| WarError::CacheWriteError {
        module: module.path.clone(),
        source: e,
    })?;

    fs::write(temp_file.path(), content).map_err(|e| WarError::CacheWriteError {
        module: module.path.clone(),
        source: e,
    })?;

    // Atomic rename into final location
    temp_file
        .persist(&info_path)
        .map_err(|e| WarError::CacheWriteError {
            module: module.path.clone(),
            source: e.error,
        })?;

    Ok(())
}

/// Generate the .info JSON content for a module.
///
/// Format:
/// ```text
/// {
///   "Version": "v1.0.0",
///   "Time": "2025-12-31T12:19:35Z",
///   "Origin": {
///     "VCS": "git",
///     "Hash": "3ba77644ce5e48f97541214eb60ac95b4eba0ba6"
///   }
/// }
/// ```
///
/// More info: https://go.dev/ref/mod#module-cache
pub fn generate_info_content(module: &VendorModule) -> Result<String, WarError> {
    #[derive(Serialize)]
    struct InfoOrigin {
        vcs: String,
        url: String,
        hash: String,
    }

    #[derive(Serialize)]
    struct InfoFile {
        version: String,
        time: DateTime<Utc>,
        origin: Option<InfoOrigin>,
    }

    // Synthesize origin data from vendor info (best-effort for offline mode)
    let origin = if module.explicit {
        Some(InfoOrigin {
            vcs: "git".to_string(),
            url: format!("https://{}", module.path),
            // Use version as placeholder hash; real hash would come from go.sum
            hash: module.version.clone(),
        })
    } else {
        None
    };

    let info = InfoFile {
        version: module.version.clone(),
        time: Utc::now(),
        origin,
    };

    serde_json::to_string_pretty(&info).map_err(|e| WarError::ConfigError {
        source: Box::new(e),
    })
}

/// Copy the .mod file from vendored source to cache, using atomic write.
///
/// The .mod file is simply the module's go.mod file. For vendored modules,
/// it lives at vendor/<module_path>/go.mod. If not found, we synthesize a
/// minimal go.mod with just the module path and version.
fn write_mod_file(module: &VendorModule, cache_dir: &Path) -> Result<(), WarError> {
    let mod_path = cache_dir.join(format!("{}.mod", module.version));

    // Try to read from vendor source first
    let content = if module.vendor_path.join("go.mod").exists() {
        fs::read_to_string(module.vendor_path.join("go.mod")).map_err(|e| {
            WarError::CacheWriteError {
                module: module.path.clone(),
                source: e,
            }
        })?
    } else {
        // Synthesize minimal go.mod if vendored copy is missing
        // This handles edge cases like indirect deps or older modules
        format!("module {}\n\ngo 1.20\n", module.path)
    };

    // Atomic write: write to temp file in same directory, then rename
    let temp_file = NamedTempFile::new_in(cache_dir).map_err(|e| WarError::CacheWriteError {
        module: module.path.clone(),
        source: e,
    })?;

    fs::write(temp_file.path(), content).map_err(|e| WarError::CacheWriteError {
        module: module.path.clone(),
        source: e,
    })?;

    // Atomic rename into final location
    temp_file
        .persist(&mod_path)
        .map_err(|e| WarError::CacheWriteError {
            module: module.path.clone(),
            source: e.error,
        })?;

    Ok(())
}

#[cfg(test)]
mod cache_tests {
    use super::*;

    #[test]
    fn test_generate_info_content_explicit_module() {
        let module = VendorModule {
            path: "github.com/gin-gonic/gin".to_string(),
            version: "v1.9.1".to_string(),
            explicit: true,
            go_version: Some("1.20".to_string()),
            packages: vec!["github.com/gin-gonic/gin".to_string()],
            vendor_path: PathBuf::new(),
        };

        let content = generate_info_content(&module).unwrap();

        // Parse JSON to verify structure instead of string matching
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();

        // Verify version field
        assert_eq!(parsed["version"].as_str().unwrap(), "v1.9.1");

        // Verify time field exists and is recent
        assert!(parsed["time"].is_string());
        let time_str = parsed["time"].as_str().unwrap();
        let parsed_time: DateTime<Utc> = time_str.parse().unwrap();
        let now = Utc::now();
        assert!((now - parsed_time).num_seconds() < 60);

        // Verify origin structure
        assert!(parsed["origin"].is_object());
        assert_eq!(parsed["origin"]["vcs"].as_str().unwrap(), "git");
        assert_eq!(
            parsed["origin"]["url"].as_str().unwrap(),
            "https://github.com/gin-gonic/gin"
        );
        assert_eq!(parsed["origin"]["hash"].as_str().unwrap(), "v1.9.1");
    }

    #[test]
    fn test_generate_info_content_implicit_module() {
        let module = VendorModule {
            path: "golang.org/x/net".to_string(),
            version: "v0.10.0".to_string(),
            explicit: false,
            go_version: None,
            packages: vec![],
            vendor_path: PathBuf::new(),
        };

        let content = generate_info_content(&module).unwrap();

        // Parse JSON to verify structure
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();

        // Verify version field
        assert_eq!(parsed["version"].as_str().unwrap(), "v0.10.0");

        // Verify time field exists
        assert!(parsed["time"].is_string());

        // Verify origin is either null or missing for implicit modules
        // (based on implementation, it's null because we use None)
        assert!(parsed["origin"].is_null());
    }

    #[test]
    fn test_write_mod_file_from_vendor_source() {
        use std::fs;
        use tempfile::tempdir;

        let cache_dir = tempdir().unwrap();
        let vendor_dir = tempdir().unwrap();

        // Create a mock vendored go.mod
        let vendor_mod = vendor_dir.path().join("go.mod");
        fs::write(
            &vendor_mod,
            "module github.com/test/lib\n\ngo 1.21\n\nrequire github.com/foo/bar v1.0.0\n",
        )
        .unwrap();

        let module = VendorModule {
            path: "github.com/test/lib".to_string(),
            version: "v1.2.3".to_string(),
            explicit: true,
            go_version: Some("1.21".to_string()),
            packages: vec![],
            vendor_path: vendor_dir.path().to_path_buf(),
        };

        write_mod_file(&module, cache_dir.path()).unwrap();

        let mod_path = cache_dir.path().join("v1.2.3.mod");
        assert!(mod_path.exists(), ".mod file not created");

        let content = fs::read_to_string(mod_path).unwrap();
        assert!(content.starts_with("module github.com/test/lib"));
        assert!(content.contains("go 1.21"));
        assert!(content.contains("github.com/foo/bar v1.0.0"));
    }

    #[test]
    fn test_write_mod_file_synthesizes_fallback() {
        use std::fs;
        use tempfile::tempdir;

        let cache_dir = tempdir().unwrap();
        let vendor_dir = tempdir().unwrap();

        // No go.mod in vendor dir → should synthesize fallback
        let module = VendorModule {
            path: "github.com/indirect/dep".to_string(),
            version: "v0.5.0".to_string(),
            explicit: false,
            go_version: None,
            packages: vec![],
            vendor_path: vendor_dir.path().to_path_buf(),
        };

        write_mod_file(&module, cache_dir.path()).unwrap();

        let mod_path = cache_dir.path().join("v0.5.0.mod");
        assert!(mod_path.exists());

        let content = fs::read_to_string(mod_path).unwrap();
        assert_eq!(content, "module github.com/indirect/dep\n\ngo 1.20\n");
    }
}
