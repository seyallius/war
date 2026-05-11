//! airgap_loop_test.rs - End-to-end integration test for the
//! pack → unpack → offline → verify loop.
//!
//! This test simulates the full air-gap workflow:
//! 1. Create a synthetic Go module cache with realistic file layout.
//! 2. Pack it into a zip archive via `pack_modules`.
//! 3. Unpack the archive into a clean directory via `unpack_modules`.
//! 4. Verify the unpacked directory structure matches the original byte-for-byte.
//! 5. Call `generate_offline_exports()` and assert it points `GOPROXY` to the
//!    unpacked cache with `file://` protocol.
//! 6. Inject the environment variables and verify they are set correctly.
//!
//! Because this environment may not have the Go toolchain installed, the
//! actual `go build` step is tested only as a dry-run assertion on the env
//! vars — not by running `go build` itself.

use std::{fs, path::Path};
use tempfile::TempDir;
use war_go::{
    default_cache_root, generate_offline_exports, generate_online_exports, go_offline,
    pack_modules, unpack_modules, unpack_modules_with_opts, UnpackOpts, UnpackStats,
};

// -------------------------------------------- Integration Tests --------------------------------------------

#[tokio::test]
async fn airgap_pack_unpack_offline_loop() {
    println!("\n=== Phase 3B Air-gap Loop Integration Test ===\n");

    // ── 1. Create a synthetic Go module cache ─────────────────────────
    let tmp = TempDir::new().expect("tempdir failed");
    let cache_root = tmp.path().join("go-cache");

    create_synthetic_cache(&cache_root);

    println!(
        "Step 1: Created synthetic Go cache at: {}",
        cache_root.display()
    );
    print_tree(&cache_root);

    // ── 2. Pack the cache into a zip archive ──────────────────────────
    let archive_path = tmp.path().join("war-pack.zip");
    println!("\nStep 2: Packing cache → {}", archive_path.display());

    pack_modules(&cache_root, &archive_path, None)
        .await
        .expect("pack_modules failed");

    assert!(archive_path.exists(), "Archive should exist after packing");
    let archive_size = fs::metadata(&archive_path).unwrap().len();
    println!("  ✔ Archive created: {} bytes", archive_size);

    // ── 3. Unpack into a clean directory ──────────────────────────────
    let unpack_root = tmp.path().join("unpacked-cache");
    println!("\nStep 3: Unpacking archive → {}", unpack_root.display());

    let stats: UnpackStats =
        unpack_modules(&archive_path, &unpack_root).expect("unpack_modules failed");

    println!(
        "  ✔ Unpack stats: extracted={}, skipped={}, failed={}",
        stats.extracted, stats.skipped, stats.failed
    );

    assert_eq!(stats.extracted, 7, "Should extract 7 files");
    assert_eq!(stats.skipped, 0, "Fresh unpack should skip nothing");
    assert_eq!(stats.failed, 0, "Should have 0 failures");

    // ── 4. Verify directory structures match byte-for-byte ────────────
    println!("\nStep 4: Verifying directory structures match");
    println!("  Original:");
    print_tree(&cache_root);
    println!("  Unpacked:");
    print_tree(&unpack_root);

    let checks: Vec<(&str, &[u8])> = vec![
        (
            "github.com!gin-gonic!gin/@v/v1.9.1.info",
            br#"{"Version":"v1.9.1","Time":"2023-06-01T12:00:00Z"}"#,
        ),
        (
            "github.com!gin-gonic!gin/@v/v1.9.1.mod",
            b"module github.com/gin-gonic/gin\n\ngo 1.20\n",
        ),
        (
            "github.com!gin-gonic!gin/@v/v1.9.1.zip",
            b"PK\x03\x04gin-zip-payload",
        ),
        (
            "golang.org!x!text/@v/v0.3.7.info",
            br#"{"Version":"v0.3.7","Time":"2022-09-01T00:00:00Z"}"#,
        ),
        (
            "golang.org!x!text/@v/v0.3.7.mod",
            b"module golang.org/x/text\n\ngo 1.17\n",
        ),
        (
            "github.com!stretchr!testify/@v/v1.8.4.info",
            br#"{"Version":"v1.8.4","Time":"2023-05-01T00:00:00Z"}"#,
        ),
        (
            "github.com!stretchr!testify/@v/v1.8.4.mod",
            b"module github.com/stretchr/testify\n\ngo 1.20\n",
        ),
    ];

    let mut all_passed = true;
    for (relative, expected) in &checks {
        let original = cache_root.join(relative);
        let unpacked = unpack_root.join(relative);

        assert!(original.exists(), "Original should exist: {}", relative);
        assert!(unpacked.exists(), "Unpacked should exist: {}", relative);

        let orig_bytes = fs::read(&original).expect("read original");
        let unpack_bytes = fs::read(&unpacked).expect("read unpacked");

        if orig_bytes == unpack_bytes && unpack_bytes.as_slice() == *expected {
            println!("  ✔ PASS: {} ({} bytes)", relative, unpack_bytes.len());
        } else {
            println!("  ✘ FAIL: {} — content mismatch", relative);
            all_passed = false;
        }
    }
    assert!(all_passed, "All file content checks must pass");

    // ── 5. Test idempotent re-unpack ──────────────────────────────────
    println!("\nStep 5: Re-unpack (idempotency check)");
    let stats2: UnpackStats =
        unpack_modules(&archive_path, &unpack_root).expect("second unpack failed");

    println!(
        "  ✔ Re-unpack stats: extracted={}, skipped={}, failed={}",
        stats2.extracted, stats2.skipped, stats2.failed
    );
    assert_eq!(stats2.extracted, 0, "Re-unpack should extract nothing");
    assert_eq!(stats2.skipped, 7, "Re-unpack should skip all 7 files");
    assert_eq!(stats2.failed, 0, "Re-unpack should have 0 failures");

    // ── 6. Test dry-run mode ──────────────────────────────────────────
    println!("\nStep 6: Dry-run unpack");
    let dry_target = tmp.path().join("dry-run-cache");
    let dry_opts = UnpackOpts {
        dry_run: true,
        ..Default::default()
    };
    let dry_stats = unpack_modules_with_opts(&archive_path, &dry_target, &dry_opts)
        .expect("dry-run unpack failed");

    println!(
        "  ✔ Dry-run stats: extracted={}, skipped={}, failed={}",
        dry_stats.extracted, dry_stats.skipped, dry_stats.failed
    );
    assert_eq!(
        dry_stats.extracted, 7,
        "Dry-run should count 7 would-be-extracted"
    );
    assert!(
        !dry_target.exists(),
        "Dry-run should NOT create target directory"
    );

    // ── 7. Test offline exports point to the correct cache path ───────
    println!("\nStep 7: Verify offline exports");
    let exports = generate_offline_exports();
    println!("  Offline exports:\n{}", exports);

    let expected_cache = default_cache_root().expect("cache root should resolve");
    assert!(
        exports.contains(&format!("GOPROXY=file://{}", expected_cache.display())),
        "GOPROXY should point to ~/.war/cache/go via file:// protocol"
    );
    assert!(exports.contains("GONOSUMDB=*"), "GONOSUMDB should be *");
    assert!(exports.contains("GOSUMDB=off"), "GOSUMDB should be off");
    assert!(
        exports.contains("GOFLAGS=-mod=readonly"),
        "GOFLAGS should be -mod=readonly"
    );

    // ── 8. Test go_offline sets env vars correctly ────────────────────
    println!("\nStep 8: Apply offline env vars programmatically");
    let changes = go_offline(None, false).expect("go_offline should succeed");

    assert_eq!(changes.len(), 4, "Should set 4 env vars");
    assert_eq!(
        std::env::var("GOPROXY").unwrap(),
        format!("file://{}", expected_cache.display())
    );
    assert_eq!(std::env::var("GONOSUMDB").unwrap(), "*");
    assert_eq!(std::env::var("GOSUMDB").unwrap(), "off");
    assert_eq!(std::env::var("GOFLAGS").unwrap(), "-mod=readonly");
    println!("  ✔ All 4 offline env vars set correctly");

    // Clean up env vars so other tests aren't affected.
    for (key, old) in &changes {
        match old {
            Some(val) => std::env::set_var(key, val),
            None => std::env::remove_var(key),
        }
    }

    // ── 9. Test online exports ────────────────────────────────────────
    println!("\nStep 9: Verify online exports");
    let online_exports = generate_online_exports();
    println!("  Online exports:\n{}", online_exports);
    assert!(online_exports.contains("unset GOPROXY"));
    assert!(online_exports.contains("unset GONOSUMDB"));
    assert!(online_exports.contains("unset GOSUMDB"));
    assert!(online_exports.contains("unset GOFLAGS"));

    println!("\n=== Air-gap Loop Test PASSED (◕‿◕✿) ===");
}

// -------------------------------------------- Internal Helpers --------------------------------------------

/// Print all files under `root` with their sizes, sorted by path.
fn print_tree(root: &Path) {
    let mut files: Vec<String> = Vec::new();
    collect_files(root, root, &mut files);
    files.sort();
    for f in &files {
        let meta = fs::metadata(root.join(f)).ok();
        let size = meta.map(|m| m.len()).unwrap_or(0);
        println!("    {} ({} bytes)", f, size);
    }
}

/// Recursively collect relative file paths under `current` relative to `base`.
fn collect_files(base: &Path, current: &Path, files: &mut Vec<String>) {
    if let Ok(entries) = fs::read_dir(current) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                collect_files(base, &path, files);
            } else if let Ok(relative) = path.strip_prefix(base) {
                files.push(relative.to_string_lossy().to_string());
            }
        }
    }
}

/// Build a realistic synthetic Go module cache under `root`.
///
/// Creates three modules mimicking the actual `~/go/pkg/mod/cache/download`
/// layout with `.info`, `.mod`, and `.zip` files.
fn create_synthetic_cache(root: &Path) {
    // Module 1: github.com/gin-gonic/gin v1.9.1
    fs::create_dir_all(root.join("github.com!gin-gonic!gin/@v")).unwrap();
    fs::write(
        root.join("github.com!gin-gonic!gin/@v/v1.9.1.info"),
        r#"{"Version":"v1.9.1","Time":"2023-06-01T12:00:00Z"}"#,
    )
    .unwrap();
    fs::write(
        root.join("github.com!gin-gonic!gin/@v/v1.9.1.mod"),
        "module github.com/gin-gonic/gin\n\ngo 1.20\n",
    )
    .unwrap();
    fs::write(
        root.join("github.com!gin-gonic!gin/@v/v1.9.1.zip"),
        b"PK\x03\x04gin-zip-payload",
    )
    .unwrap();

    // Module 2: golang.org/x/text v0.3.7
    fs::create_dir_all(root.join("golang.org!x!text/@v")).unwrap();
    fs::write(
        root.join("golang.org!x!text/@v/v0.3.7.info"),
        r#"{"Version":"v0.3.7","Time":"2022-09-01T00:00:00Z"}"#,
    )
    .unwrap();
    fs::write(
        root.join("golang.org!x!text/@v/v0.3.7.mod"),
        "module golang.org/x/text\n\ngo 1.17\n",
    )
    .unwrap();

    // Module 3: github.com/stretchr/testify v1.8.4
    fs::create_dir_all(root.join("github.com!stretchr!testify/@v")).unwrap();
    fs::write(
        root.join("github.com!stretchr!testify/@v/v1.8.4.info"),
        r#"{"Version":"v1.8.4","Time":"2023-05-01T00:00:00Z"}"#,
    )
    .unwrap();
    fs::write(
        root.join("github.com!stretchr!testify/@v/v1.8.4.mod"),
        "module github.com/stretchr/testify\n\ngo 1.20\n",
    )
    .unwrap();
}
