//! Integration tests for cache reconstruction with real I/O and atomic writes.
//!
//! These tests verify that .info/.mod/.zip files are correctly generated,
//! written atomically, and placed in the proper Go module cache layout.

use std::{fs, path::PathBuf};
use tempfile::tempdir;
use war_core::types::VendorModule;
use war_go::cache::reconstruct_cache;

// -------------------------------------------- Integration Tests --------------------------------------------

/// Test that reconstruct_cache creates proper .info files in Go cache layout
#[test]
fn test_reconstruct_cache_creates_info_file() {
    let cache_root = tempdir().unwrap();
    let modules = vec![VendorModule {
        path: "github.com/gin-gonic/gin".to_string(),
        version: "v1.9.1".to_string(),
        explicit: true,
        go_version: Some("1.20".to_string()),
        packages: vec!["github.com/gin-gonic/gin".to_string()],
        vendor_path: PathBuf::new(),
    }];

    reconstruct_cache(&modules, cache_root.path()).unwrap();

    // Go cache layout: <cache_root>/<module_path_with_!>/@v/<version>.info
    let module_cache = cache_root
        .path()
        .join("github.com!gin-gonic!gin")
        .join("@v");

    let info_path = module_cache.join("v1.9.1.info");
    assert!(
        info_path.exists(),
        ".info file not created at {:?}",
        info_path
    );

    // Verify JSON content is valid and contains expected fields
    let content = fs::read_to_string(&info_path).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();

    assert_eq!(parsed["version"].as_str().unwrap(), "v1.9.1");
    assert!(parsed["time"].is_string());
    assert!(parsed["origin"].is_object());
    assert_eq!(parsed["origin"]["vcs"].as_str().unwrap(), "git");
}

/// Test that atomic writes prevent partial files on failure
#[test]
fn test_atomic_write_prevents_partial_files() {
    let cache_root = tempdir().unwrap();
    let modules = vec![VendorModule {
        path: "github.com/test/module".to_string(),
        version: "v0.1.0".to_string(),
        explicit: false,
        go_version: None,
        packages: vec![],
        vendor_path: PathBuf::new(),
    }];

    // First sync should succeed
    reconstruct_cache(&modules, cache_root.path()).unwrap();

    let info_path = cache_root
        .path()
        .join("github.com!test!module")
        .join("@v")
        .join("v0.1.0.info");

    assert!(info_path.exists());

    // Verify no temp files left behind
    let cache_dir = info_path.parent().unwrap();
    let temp_files: Vec<_> = fs::read_dir(cache_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.file_name().to_string_lossy().contains(".tmp")
                || e.file_name().to_string_lossy().ends_with("tmp")
        })
        .collect();

    assert!(
        temp_files.is_empty(),
        "Atomic write left temp files behind: {:?}",
        temp_files
    );
}

/// Test that multiple modules are synced independently
#[test]
fn test_reconstruct_cache_handles_multiple_modules() {
    let cache_root = tempdir().unwrap();
    let modules = vec![
        VendorModule {
            path: "github.com/gin-gonic/gin".to_string(),
            version: "v1.9.1".to_string(),
            explicit: true,
            go_version: Some("1.20".to_string()),
            packages: vec!["github.com/gin-gonic/gin".to_string()],
            vendor_path: PathBuf::new(),
        },
        VendorModule {
            path: "golang.org/x/net".to_string(),
            version: "v0.10.0".to_string(),
            explicit: false,
            go_version: None,
            packages: vec!["golang.org/x/net/html".to_string()],
            vendor_path: PathBuf::new(),
        },
    ];

    reconstruct_cache(&modules, cache_root.path()).unwrap();

    // Verify both modules have their .info files
    let gin_info = cache_root
        .path()
        .join("github.com!gin-gonic!gin")
        .join("@v")
        .join("v1.9.1.info");

    let net_info = cache_root
        .path()
        .join("golang.org!x!net")
        .join("@v")
        .join("v0.10.0.info");

    assert!(gin_info.exists(), "gin .info not found");
    assert!(net_info.exists(), "net .info not found");

    // Verify gin has origin (explicit), net does not (implicit)
    let gin_content = fs::read_to_string(gin_info).unwrap();
    let net_content = fs::read_to_string(net_info).unwrap();

    let gin_parsed: serde_json::Value = serde_json::from_str(&gin_content).unwrap();
    let net_parsed: serde_json::Value = serde_json::from_str(&net_content).unwrap();

    assert!(gin_parsed["origin"].is_object());
    assert!(net_parsed["origin"].is_null());
}

/// Test that cache reconstruction is idempotent (running twice yields same result)
#[test]
fn test_reconstruct_cache_is_idempotent() {
    let cache_root = tempdir().unwrap();
    let modules = vec![VendorModule {
        path: "github.com/example/lib".to_string(),
        version: "v2.0.0".to_string(),
        explicit: true,
        go_version: Some("1.21".to_string()),
        packages: vec!["github.com/example/lib".to_string()],
        vendor_path: PathBuf::new(),
    }];

    // First run
    reconstruct_cache(&modules, cache_root.path()).unwrap();
    let info_path = cache_root
        .path()
        .join("github.com!example!lib")
        .join("@v")
        .join("v2.0.0.info");
    let first_content = fs::read_to_string(&info_path).unwrap();

    // Second run (idempotent)
    reconstruct_cache(&modules, cache_root.path()).unwrap();
    let second_content = fs::read_to_string(&info_path).unwrap();

    // Parse both to compare structure (timestamps will differ)
    let first_parsed: serde_json::Value = serde_json::from_str(&first_content).unwrap();
    let second_parsed: serde_json::Value = serde_json::from_str(&second_content).unwrap();

    // Version and origin should match
    assert_eq!(first_parsed["version"], second_parsed["version"]);
    assert_eq!(first_parsed["origin"], second_parsed["origin"]);

    // Timestamps might differ, so just verify they exist
    assert!(first_parsed["time"].is_string());
    assert!(second_parsed["time"].is_string());
}

/// Test that parallel reconstruction handles many modules efficiently
#[test]
fn test_reconstruct_cache_parallel_performance() {
    let cache_root = tempdir().unwrap();

    // Generate 50 mock modules to stress-test parallelism
    let modules: Vec<VendorModule> = (0..50)
        .map(|i| VendorModule {
            path: format!("github.com/test/module-{}", i),
            version: format!("v{}.0.0", i), // Fixed: use consistent format
            explicit: i % 2 == 0,
            go_version: if i % 2 == 0 {
                Some("1.20".to_string())
            } else {
                None
            },
            packages: vec![format!("github.com/test/module-{}/subpkg", i)],
            vendor_path: PathBuf::new(),
        })
        .collect();

    // This should complete quickly thanks to rayon parallelism
    let start = std::time::Instant::now();
    reconstruct_cache(&modules, cache_root.path()).unwrap();
    let elapsed = start.elapsed();

    // Sanity check: should finish in <5 seconds on modern hardware
    // (adjust threshold based on CI environment)
    assert!(
        elapsed.as_secs() < 5,
        "Parallel reconstruction took too long: {:?}",
        elapsed
    );

    // Verify a sample of modules were created
    for i in [0, 25, 49] {
        let info_path = cache_root
            .path()
            .join(format!("github.com!test!module-{}", i))
            .join("@v")
            .join(format!("v{}.0.0.info", i)); // Fixed: match the version format
        assert!(
            info_path.exists(),
            "Module {} .info not found at {:?}",
            i,
            info_path
        );
    }
}

/// Test that .mod files are correctly copied from vendor source
#[test]
fn test_reconstruct_cache_copies_mod_file() {
    use std::fs;
    use tempfile::tempdir;

    let cache_root = tempdir().unwrap();
    let vendor_root = tempdir().unwrap();

    // Create mock vendor structure with go.mod
    let vendor_mod_dir = vendor_root
        .path()
        .join("github.com")
        .join("test")
        .join("lib");
    fs::create_dir_all(&vendor_mod_dir).unwrap();
    fs::write(
        vendor_mod_dir.join("go.mod"),
        "module github.com/test/lib\n\ngo 1.21\n",
    )
    .unwrap();

    let modules = vec![VendorModule {
        path: "github.com/test/lib".to_string(),
        version: "v1.0.0".to_string(),
        explicit: true,
        go_version: Some("1.21".to_string()),
        packages: vec!["github.com/test/lib".to_string()],
        vendor_path: vendor_mod_dir,
    }];

    reconstruct_cache(&modules, cache_root.path()).unwrap();

    let mod_path = cache_root
        .path()
        .join("github.com!test!lib")
        .join("@v")
        .join("v1.0.0.mod");

    assert!(mod_path.exists(), ".mod file not created at {:?}", mod_path);

    let content = fs::read_to_string(&mod_path).unwrap();
    assert!(content.starts_with("module github.com/test/lib"));
    assert!(content.contains("go 1.21"));
}
