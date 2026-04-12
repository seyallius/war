//! cache - Reconstruct ~/go/pkg/mod cache from vendored modules.
//!
//! Handles the zero-network sync: generating .info, .mod, and .zip
//! files per module version, with parallel processing and atomic writes.

use chrono::{DateTime, Utc};
use rayon::prelude::*;
use serde::Serialize;
use std::{
    fs, io,
    path::{self, Path, PathBuf},
    sync::Mutex,
};
use tempfile::NamedTempFile;
use war_core::{types::VendorModule, WarError};
use zip::{
    write::{FileOptions, SimpleFileOptions},
    ZipWriter,
};
// -------------------------------------------- Public API --------------------------------------------

/// Reconstruct Go module cache entries from vendored source.
///
/// * modules: list of VendorModule from vendor/modules.txt parsing
/// * cache_root: target directory (usually `~/.go/pkg/mod/cache/download`)
/// * vendor_root: root of the vendor directory (e.g., `./vendor` or absolute path)
///
/// Uses rayon for parallel processing: each module's `.info`/`.mod`/`.zip` generation
/// runs concurrently, significantly speeding up cache reconstruction for large
/// vendor directories (200+ modules).
pub fn reconstruct_cache(
    modules: &[VendorModule],
    cache_root: &Path,
    vendor_root: &Path,
) -> Result<(), WarError> {
    // Collect errors from parallel iteration in a thread-safe way
    let errors = Mutex::new(Vec::<(String, WarError)>::new());

    modules.par_iter().try_for_each(|module| {
        match sync_module_cache(module, cache_root, vendor_root) {
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
fn sync_module_cache(
    module: &VendorModule,
    cache_root: &Path,
    vendor_root: &Path,
) -> Result<(), WarError> {
    // Go cache layout: <cache_root>/<module_path_with_!>/@v/
    let module_cache_dir = cache_root.join(module.path.replace('/', "!")).join("@v");
    fs::create_dir_all(&module_cache_dir).map_err(|e| WarError::CacheWriteError {
        module: module.path.clone(),
        source: e,
    })?;

    // Generate and write .info file (atomic)
    write_info_file(module, &module_cache_dir)?;

    // Copy .mod file from vendor source (atomic)
    write_mod_file(module, &module_cache_dir, vendor_root)?;

    // Create .zip archive from vendored source (atomic, CPU-bound)
    write_zip_file(module, &module_cache_dir, vendor_root)?;

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
fn write_mod_file(
    module: &VendorModule,
    cache_dir: &Path,
    vendor_root: &Path,
) -> Result<(), WarError> {
    let mod_path = cache_dir.join(format!("{}.mod", module.version));

    // Compute vendor source path from module path
    let vendor_src = vendor_root
        .join(module.path.replace('/', path::MAIN_SEPARATOR_STR))
        .join("go.mod");

    let content = if vendor_src.exists() {
        fs::read_to_string(&vendor_src).map_err(|e| WarError::CacheWriteError {
            module: module.path.clone(),
            source: e,
        })?
    } else {
        // Fallback: synthesize minimal go.mod if vendored copy is missing
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

/// Create a .zip archive from vendored source files, matching Go's cache format.
///
/// Go expects the zip to contain files under a root directory named
/// `<module>@<version>/`.
///
/// This function walks the vendor source tree, adds each file to the zip
/// with proper relative paths, and writes atomically to prevent partial archives.
fn write_zip_file(
    module: &VendorModule,
    cache_dir: &Path,
    vendor_root: &Path,
) -> Result<(), WarError> {
    let zip_path = cache_dir.join(format!("{}.zip", module.version));

    // Compute vendor source directory for this module
    let vendor_src_dir = vendor_root.join(module.path.replace('/', path::MAIN_SEPARATOR_STR));

    // Atomic write: create temp file in same directory, then rename
    let temp_file = NamedTempFile::new_in(cache_dir).map_err(|e| WarError::CacheWriteError {
        module: module.path.clone(),
        source: e,
    })?;

    // Create ZIP writer with deflate compression
    let file = fs::File::create(temp_file.path()).map_err(|e| WarError::CacheWriteError {
        module: module.path.clone(),
        source: e,
    })?;

    let mut zip = ZipWriter::new(file);
    let options: FileOptions<'_, ()> = SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .unix_permissions(0o755);

    // Root directory inside zip: <module>@<version>/
    let zip_root = format!("{}@{}/", module.path, module.version);

    // Walk the vendor source directory and add files to zip
    walk_vendor_source(&vendor_src_dir, &zip_root, &mut zip, options)?;

    // Finish writing the zip archive
    zip.finish().map_err(|e| WarError::ZipCreationError {
        module: module.path.clone(),
        source: e,
    })?;

    // Atomic rename into final location
    temp_file
        .persist(&zip_path)
        .map_err(|e| WarError::CacheWriteError {
            module: module.path.clone(),
            source: e.error,
        })?;

    Ok(())
}

/// Recursively walk vendor source directory and add files to ZIP archive.
///
/// Preserves directory structure relative to the module root, and skips
/// vendor-specific metadata files that Go doesn't expect in the cache zip.
fn walk_vendor_source(
    src_dir: &Path,
    zip_root: &str,
    zip: &mut ZipWriter<fs::File>,
    options: FileOptions<'_, ()>,
) -> Result<(), WarError> {
    walk_vendor_source_internal(src_dir, src_dir, zip_root, zip, options)
}

/// Internal recursive implementation that tracks the original base directory.
fn walk_vendor_source_internal(
    current_dir: &Path,
    base_dir: &Path,
    zip_root: &str,
    zip: &mut ZipWriter<fs::File>,
    options: FileOptions<'_, ()>,
) -> Result<(), WarError> {
    let src = fs::read_dir(current_dir).map_err(|e| WarError::CacheWriteError {
        module: current_dir.to_string_lossy().to_string(),
        source: e,
    })?;

    for entry in src {
        let entry = entry.map_err(|e| WarError::CacheWriteError {
            module: current_dir.to_string_lossy().to_string(),
            source: e,
        })?;

        let path = entry.path();
        let file_name = path.file_name().unwrap_or_default().to_string_lossy();

        // Skip Go-specific metadata that shouldn't be in the cache zip
        if file_name == "go.mod" || file_name == "go.sum" || file_name == "vendor" {
            continue;
        }

        // Compute relative path from base directory
        let relative_path = path
            .strip_prefix(base_dir)
            .map_err(|e| WarError::CacheWriteError {
                module: current_dir.to_string_lossy().to_string(),
                source: io::Error::new(io::ErrorKind::InvalidInput, e),
            })?;

        let zip_path = PathBuf::from(zip_root)
            .join(relative_path)
            .to_string_lossy()
            .replace('\\', "/"); // ZIP spec requires forward slashes

        if path.is_dir() {
            // Add directory entry to zip
            zip.add_directory(&zip_path, options)
                .map_err(|e| WarError::ZipCreationError {
                    module: current_dir.to_string_lossy().to_string(),
                    source: e,
                })?;
            // Recurse into subdirectory
            walk_vendor_source_internal(&path, base_dir, zip_root, zip, options)?;
        } else {
            // Add file entry to zip
            zip.start_file(&zip_path, options)
                .map_err(|e| WarError::ZipCreationError {
                    module: current_dir.to_string_lossy().to_string(),
                    source: e,
                })?;

            let mut src_file = fs::File::open(&path).map_err(|e| WarError::CacheWriteError {
                module: path.to_string_lossy().to_string(),
                source: e,
            })?;

            io::copy(&mut src_file, zip).map_err(|e| WarError::CacheWriteError {
                module: path.to_string_lossy().to_string(),
                source: e,
            })?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod cache_tests {
    use super::*;
    use std::io::Read;
    use tempfile::tempdir;

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

        // Create the proper vendor directory structure matching the module path
        let module_vendor_path = vendor_dir.path().join("github.com/test/lib");
        fs::create_dir_all(&module_vendor_path).unwrap();

        // Create a mock vendored go.mod in the correct location
        let vendor_mod = module_vendor_path.join("go.mod");
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
            vendor_path: module_vendor_path,
        };

        write_mod_file(&module, cache_dir.path(), vendor_dir.path()).unwrap();

        let mod_path = cache_dir.path().join("v1.2.3.mod");
        assert!(mod_path.exists(), ".mod file not created at {:?}", mod_path);

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

        write_mod_file(&module, cache_dir.path(), vendor_dir.path()).unwrap();

        let mod_path = cache_dir.path().join("v0.5.0.mod");
        assert!(mod_path.exists());

        let content = fs::read_to_string(mod_path).unwrap();
        assert_eq!(content, "module github.com/indirect/dep\n\ngo 1.20\n");
    }

    #[test]
    fn test_write_zip_file_creates_valid_archive() {
        use super::*;

        let cache_dir = tempdir().unwrap();
        let vendor_root = tempdir().unwrap();

        // Create mock vendored source structure
        let vendor_mod_dir = vendor_root
            .path()
            .join("github.com")
            .join("test")
            .join("lib");
        fs::create_dir_all(&vendor_mod_dir).unwrap();
        fs::write(vendor_mod_dir.join("main.go"), "package main\n").unwrap();
        fs::write(vendor_mod_dir.join("util.go"), "package main\n").unwrap();
        fs::create_dir_all(vendor_mod_dir.join("internal")).unwrap();
        fs::write(
            vendor_mod_dir.join("internal").join("helper.go"),
            "package internal\n",
        )
        .unwrap();
        // These should be skipped
        fs::write(vendor_mod_dir.join("go.mod"), "module test\n").unwrap();
        fs::write(vendor_mod_dir.join("go.sum"), "hash\n").unwrap();

        let module = VendorModule {
            path: "github.com/test/lib".to_string(),
            version: "v1.0.0".to_string(),
            explicit: true,
            go_version: Some("1.21".to_string()),
            packages: vec!["github.com/test/lib".to_string()],
            vendor_path: vendor_mod_dir.clone(),
        };

        write_zip_file(&module, cache_dir.path(), vendor_root.path()).unwrap();

        let zip_path = cache_dir.path().join("v1.0.0.zip");
        assert!(zip_path.exists(), ".zip file not created at {:?}", zip_path);

        // Verify zip contents
        let zip_file = fs::File::open(&zip_path).unwrap();
        let mut archive = zip::ZipArchive::new(zip_file).unwrap();

        // Check that the zip contains the expected files
        // Note: The root directory entry might not be explicitly stored, so check for files instead
        let expected_paths = [
            "github.com/test/lib@v1.0.0/main.go",
            "github.com/test/lib@v1.0.0/util.go",
            "github.com/test/lib@v1.0.0/internal/helper.go",
        ];

        for expected_path in expected_paths {
            assert!(
                archive.by_name(expected_path).is_ok(),
                "Expected file {} not found in zip",
                expected_path
            );
        }

        // Should NOT contain go.mod/go.sum
        assert!(
            archive
                .by_name("github.com/test/lib@v1.0.0/go.mod")
                .is_err(),
            "go.mod should not be in zip"
        );
        assert!(
            archive
                .by_name("github.com/test/lib@v1.0.0/go.sum")
                .is_err(),
            "go.sum should not be in zip"
        );

        // Verify file contents
        let mut main_file = archive
            .by_name("github.com/test/lib@v1.0.0/main.go")
            .unwrap();
        let mut contents = String::new();
        main_file.read_to_string(&mut contents).unwrap();
        assert_eq!(contents, "package main\n");
    }
}
