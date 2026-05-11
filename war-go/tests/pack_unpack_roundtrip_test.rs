//! Real-world validation: pack a synthetic Go cache, unpack it into a clean
//! directory, and verify the directory structures match exactly.

use std::{fs, path::Path};
use tempfile::TempDir;
use war_go::{pack_modules, unpack_modules, UnpackStats};

// --------------------------------------------- Integration Tests ---------------------------------------------

#[tokio::test]
async fn pack_then_unpack_roundtrip() {
    println!("=== Phase 3A Real-World Validation ===\n");

    // 1. Create a synthetic Go module cache directory (mimicking ~/.war/cache/go/)
    let tmp = TempDir::new().expect("tempdir failed");
    let cache_root = tmp.path().join("go-cache");

    // Simulate Go cache layout with `!` separators
    fs::create_dir_all(cache_root.join("github.com!gin-gonic!gin/@v")).unwrap();
    fs::create_dir_all(cache_root.join("golang.org!x!text/@v")).unwrap();
    fs::create_dir_all(cache_root.join("github.com!stretchr!testify/@v")).unwrap();

    // Write cache files
    fs::write(
        cache_root.join("github.com!gin-gonic!gin/@v/v1.9.1.info"),
        r#"{"Version":"v1.9.1","Time":"2023-01-01T00:00:00Z"}"#,
    )
    .unwrap();
    fs::write(
        cache_root.join("github.com!gin-gonic!gin/@v/v1.9.1.mod"),
        "module github.com/gin-gonic/gin\n\ngo 1.20\n",
    )
    .unwrap();
    fs::write(
        cache_root.join("github.com!gin-gonic!gin/@v/v1.9.1.zip"),
        b"PK\x03\x04fake-zip-content-gin",
    )
    .unwrap();

    fs::write(
        cache_root.join("golang.org!x!text/@v/v0.3.7.info"),
        r#"{"Version":"v0.3.7","Time":"2022-06-01T00:00:00Z"}"#,
    )
    .unwrap();
    fs::write(
        cache_root.join("golang.org!x!text/@v/v0.3.7.mod"),
        "module golang.org/x/text\n\ngo 1.17\n",
    )
    .unwrap();

    fs::write(
        cache_root.join("github.com!stretchr!testify/@v/v1.8.4.info"),
        r#"{"Version":"v1.8.4","Time":"2023-06-01T00:00:00Z"}"#,
    )
    .unwrap();
    fs::write(
        cache_root.join("github.com!stretchr!testify/@v/v1.8.4.mod"),
        "module github.com/stretchr/testify\n\ngo 1.20\n",
    )
    .unwrap();

    println!(
        "Step 1: Created synthetic Go cache at: {}",
        cache_root.display()
    );
    print_tree(&cache_root);

    // 2. Pack the cache using pack_modules
    let archive_path = tmp.path().join("war-pack.zip");
    println!("\nStep 2: Packing cache to: {}", archive_path.display());

    pack_modules(&cache_root, &archive_path, None)
        .await
        .expect("pack_modules failed");

    assert!(archive_path.exists(), "Archive should exist after packing");
    let archive_size = fs::metadata(&archive_path).unwrap().len();
    println!("  Archive created: {} bytes", archive_size);

    // 3. Unpack into a clean directory
    let unpack_root = tmp.path().join("unpacked-cache");
    println!("\nStep 3: Unpacking archive to: {}", unpack_root.display());

    let stats: UnpackStats =
        unpack_modules(&archive_path, &unpack_root).expect("unpack_modules failed");

    println!(
        "  Unpack stats: extracted={}, skipped={}, failed={}",
        stats.extracted, stats.skipped, stats.failed
    );

    assert_eq!(stats.extracted, 7, "Should extract 7 files");
    assert_eq!(stats.skipped, 0, "Should skip 0 files on fresh unpack");
    assert_eq!(stats.failed, 0, "Should have 0 failures");

    // 4. Verify directory structures match
    println!("\nStep 4: Verifying directory structures match");
    println!("  Original cache:");
    print_tree(&cache_root);
    println!("  Unpacked cache:");
    print_tree(&unpack_root);

    // Verify specific files exist with correct content
    let checks: Vec<(&str, &[u8])> = vec![
        (
            "github.com!gin-gonic!gin/@v/v1.9.1.info",
            br#"{"Version":"v1.9.1","Time":"2023-01-01T00:00:00Z"}"#,
        ),
        (
            "github.com!gin-gonic!gin/@v/v1.9.1.mod",
            b"module github.com/gin-gonic/gin\n\ngo 1.20\n",
        ),
        (
            "github.com!gin-gonic!gin/@v/v1.9.1.zip",
            b"PK\x03\x04fake-zip-content-gin",
        ),
        (
            "golang.org!x!text/@v/v0.3.7.info",
            br#"{"Version":"v0.3.7","Time":"2022-06-01T00:00:00Z"}"#,
        ),
        (
            "golang.org!x!text/@v/v0.3.7.mod",
            b"module golang.org/x/text\n\ngo 1.17\n",
        ),
        (
            "github.com!stretchr!testify/@v/v1.8.4.info",
            br#"{"Version":"v1.8.4","Time":"2023-06-01T00:00:00Z"}"#,
        ),
        (
            "github.com!stretchr!testify/@v/v1.8.4.mod",
            b"module github.com/stretchr/testify\n\ngo 1.20\n",
        ),
    ];

    let mut all_passed = true;
    for (relative, _expected_content) in &checks {
        let unpacked_path = unpack_root.join(relative);
        let original_path = cache_root.join(relative);

        assert!(
            unpacked_path.exists(),
            "Unpacked file should exist: {}",
            relative
        );
        assert!(
            original_path.exists(),
            "Original file should exist: {}",
            relative
        );

        let unpacked_content = fs::read(&unpacked_path).expect("read unpacked");
        let original_content = fs::read(&original_path).expect("read original");

        if unpacked_content == original_content {
            println!(
                "  PASS: {} ({} bytes, content matches)",
                relative,
                unpacked_content.len()
            );
        } else {
            println!(
                "  FAIL: {} (size mismatch: original={} unpacked={})",
                relative,
                original_content.len(),
                unpacked_content.len()
            );
            all_passed = false;
        }
    }

    assert!(all_passed, "All file content checks should pass");

    // 5. Test idempotency: re-unpack should skip all files
    println!("\nStep 5: Testing idempotency (re-unpack into same directory)");
    let stats2: UnpackStats =
        unpack_modules(&archive_path, &unpack_root).expect("second unpack_modules failed");

    println!(
        "  Re-unpack stats: extracted={}, skipped={}, failed={}",
        stats2.extracted, stats2.skipped, stats2.failed
    );

    assert_eq!(stats2.extracted, 0, "Re-unpack should extract 0 files");
    assert_eq!(stats2.skipped, 7, "Re-unpack should skip 7 files");
    assert_eq!(stats2.failed, 0, "Re-unpack should have 0 failures");

    // Final verdict
    println!("\n=== Validation Result ===");
    println!("ALL CHECKS PASSED! Pack -> Unpack roundtrip is correct.");
    println!("  - 7 files packed and unpacked successfully");
    println!("  - File content matches original cache");
    println!("  - Go cache layout (! separators) correctly restored");
    println!("  - Idempotent re-extraction works (size-based skip)");
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
