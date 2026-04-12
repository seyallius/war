//! Integration tests for war go offline orchestration.
//!
//! Verifies vendor path resolution, environment variable management,
//! cache reconstruction, and `war.lock` updates in isolated temp directories.
//!
//! These tests require Go toolchain and internet access.
//! Run with: `cargo test -p war-go offline_integration_test -- --ignored`

use std::{env, fs};
use tempfile::tempdir;
use war_go::offline::go_offline;

// -------------------------------------------- Integration Tests --------------------------------------------

/// Test that go_offline correctly sets env vars, updates `war.lock`, and triggers cache reconstruction.
#[test]
#[ignore] // Requires internet + go CLI
fn test_go_offline_sets_env_and_reconstructs_cache() {
    // Setup isolated directories
    let temp_home = tempdir().unwrap();
    let vendor_root = tempdir().unwrap();
    let cache_root = tempdir().unwrap();

    // Save original environment for cleanup
    let orig_home = env::var("HOME").ok();
    let orig_cache = env::var("GOMODCACHE").ok();
    let orig_goflags = env::var("GOFLAGS").ok();
    let orig_goproxy = env::var("GOPROXY").ok();
    let orig_gosumdb = env::var("GOSUMDB").ok();

    // Override for isolation
    env::set_var("HOME", temp_home.path());
    env::set_var("GOMODCACHE", cache_root.path());

    // Create mock vendor structure
    let modules_txt = "\
# github.com/test/lib v1.0.0
## explicit; go 1.20
github.com/test/lib
";
    fs::write(vendor_root.path().join("modules.txt"), modules_txt).unwrap();

    let mod_src = vendor_root
        .path()
        .join("github.com")
        .join("test")
        .join("lib");
    fs::create_dir_all(&mod_src).unwrap();
    fs::write(
        mod_src.join("go.mod"),
        "module github.com/test/lib\n\ngo 1.20\n",
    )
    .unwrap();
    fs::write(mod_src.join("main.go"), "package main\n").unwrap();

    // Execute go_offline
    let changes = go_offline(Some(vendor_root.path().to_path_buf()), false).unwrap();

    // Assert environment variables were set
    assert_eq!(env::var("GOFLAGS").unwrap(), "-mod=vendor");
    assert_eq!(env::var("GOPROXY").unwrap(), "off");
    assert_eq!(env::var("GOSUMDB").unwrap(), "off");

    // Assert war.lock was created and updated
    let lock_path = temp_home.path().join(".war").join("war.lock");
    assert!(lock_path.exists());
    let content = fs::read_to_string(&lock_path).unwrap();
    assert!(content.contains("last_vendor_path"));
    assert!(content.contains("last_sync_timestamp"));
    assert!(content.contains("go_version"));

    // Assert cache reconstruction created expected files
    let cache_module_dir = cache_root.path().join("github.com!test!lib").join("@v");
    assert!(cache_module_dir.join("v1.0.0.info").exists());
    assert!(cache_module_dir.join("v1.0.0.mod").exists());
    assert!(cache_module_dir.join("v1.0.0.zip").exists());

    // Verify returned env changes match expectations
    assert_eq!(changes.len(), 3);
    assert!(changes.iter().any(|(k, _)| k == "GOFLAGS"));
    assert!(changes.iter().any(|(k, _)| k == "GOPROXY"));
    assert!(changes.iter().any(|(k, _)| k == "GOSUMDB"));

    // Cleanup environment
    restore_env(orig_home, "HOME");
    restore_env(orig_cache, "GOMODCACHE");
    restore_env(orig_goflags, "GOFLAGS");
    restore_env(orig_goproxy, "GOPROXY");
    restore_env(orig_gosumdb, "GOSUMDB");
}

// -------------------------------------------- Internal Helpers --------------------------------------------

/// Helper to restore or remove environment variables after test execution.
fn restore_env(original: Option<String>, key: &str) {
    match original {
        Some(val) => env::set_var(key, val),
        None => env::remove_var(key),
    }
}
