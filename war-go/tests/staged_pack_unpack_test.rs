//! staged_pack_unpack_test.rs - Integration test for the staged module cart.
//!
//! Validates the full `stage → pack --staged → unpack --staged` loop:
//! 1. Create a synthetic Go module cache with three modules.
//! 2. Simulate staging only two of the three modules.
//! 3. Pack with staged filter → verify only staged entries are in the zip.
//! 4. Unpack with staged filter → verify only staged modules land on disk.
//! 5. Verify unstaged module is absent from both archive and unpacked dir.

use std::fs;
use std::path::Path;
use tempfile::TempDir;
use war_go::{pack_modules, unpack_modules, unpack_modules_with_opts, UnpackOpts};
use zip::read::ZipArchive;

/// Build a synthetic Go cache directory with three modules.
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

    // Module 3: github.com/stretchr/testify v1.8.4 (will NOT be staged)
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

/// Collect all relative file paths under `root`, sorted.
fn collect_files(root: &Path) -> Vec<String> {
    let mut files = Vec::new();
    collect_files_recursive(root, root, &mut files);
    files.sort();
    files
}

fn collect_files_recursive(base: &Path, current: &Path, files: &mut Vec<String>) {
    if let Ok(entries) = fs::read_dir(current) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                collect_files_recursive(base, &path, files);
            } else if let Ok(relative) = path.strip_prefix(base) {
                files.push(relative.to_string_lossy().to_string());
            }
        }
    }
}

/// List all entry names in a zip archive.
fn list_zip_entries(path: &Path) -> Vec<String> {
    let file = fs::File::open(path).unwrap();
    let mut archive = ZipArchive::new(file).unwrap();
    let mut names = Vec::new();
    for i in 0..archive.len() {
        let entry = archive.by_index(i).unwrap();
        names.push(entry.name().to_string());
    }
    names.sort();
    names
}

#[tokio::test]
async fn staged_pack_only_includes_staged_modules() {
    println!("\n=== Phase 4 Staged Pack/Unpack Integration Test ===\n");

    // ── 1. Create synthetic cache with 3 modules ──────────────────
    let tmp = TempDir::new().expect("tempdir failed");
    let cache_root = tmp.path().join("go-cache");
    create_synthetic_cache(&cache_root);

    println!("Step 1: Created synthetic cache at: {}", cache_root.display());
    let all_files = collect_files(&cache_root);
    for f in &all_files {
        println!("    {}", f);
    }
    assert_eq!(all_files.len(), 7, "Should have 7 files total");

    // ── 2. Pack with staged filter (only gin + text) ──────────────
    let staged_filter: Vec<(String, String)> = vec![
        ("github.com/gin-gonic/gin".to_string(), "v1.9.1".to_string()),
        ("golang.org/x/text".to_string(), "v0.3.7".to_string()),
    ];

    let staged_archive = tmp.path().join("staged-pack.zip");
    println!(
        "\nStep 2: Packing with staged filter ({} modules) → {}",
        staged_filter.len(),
        staged_archive.display()
    );

    pack_modules(&cache_root, &staged_archive, Some(staged_filter.clone()))
        .await
        .expect("staged pack failed");

    assert!(staged_archive.exists(), "Staged archive should exist");

    let staged_zip_entries = list_zip_entries(&staged_archive);
    println!("  Staged archive entries ({}):", staged_zip_entries.len());
    for name in &staged_zip_entries {
        println!("    {}", name);
    }

    // Should have 5 entries: 3 for gin + 2 for text (testify excluded)
    assert_eq!(
        staged_zip_entries.len(),
        5,
        "Staged archive should contain 5 entries (gin=3 + text=2), got {}",
        staged_zip_entries.len()
    );

    // Verify testify is NOT in the staged archive
    for name in &staged_zip_entries {
        assert!(
            !name.contains("stretchr"),
            "testify should NOT be in staged archive, found: {}",
            name
        );
    }

    // ── 3. Pack without filter (all modules) for comparison ───────
    let full_archive = tmp.path().join("full-pack.zip");
    pack_modules(&cache_root, &full_archive, None)
        .await
        .expect("full pack failed");

    let full_zip_entries = list_zip_entries(&full_archive);
    assert_eq!(full_zip_entries.len(), 7, "Full archive should have 7 entries");

    println!(
        "  Full archive entries: {} (staged: {})",
        full_zip_entries.len(),
        staged_zip_entries.len()
    );

    // ── 4. Unpack staged archive into clean directory ─────────────
    let unpack_root = tmp.path().join("unpacked-staged");
    println!(
        "\nStep 4: Unpacking staged archive → {}",
        unpack_root.display()
    );

    let stats = unpack_modules(&staged_archive, &unpack_root).expect("unpack failed");

    println!(
        "  Unpack stats: extracted={}, skipped={}, failed={}",
        stats.extracted, stats.skipped, stats.failed
    );

    assert_eq!(stats.extracted, 5, "Should extract 5 staged files");
    assert_eq!(stats.failed, 0, "Should have 0 failures");

    // Verify gin and text exist, testify does NOT
    assert!(
        unpack_root
            .join("github.com!gin-gonic!gin/@v/v1.9.1.info")
            .exists(),
        "gin info should exist"
    );
    assert!(
        unpack_root
            .join("github.com!gin-gonic!gin/@v/v1.9.1.mod")
            .exists(),
        "gin mod should exist"
    );
    assert!(
        unpack_root
            .join("github.com!gin-gonic!gin/@v/v1.9.1.zip")
            .exists(),
        "gin zip should exist"
    );
    assert!(
        unpack_root
            .join("golang.org!x!text/@v/v0.3.7.info")
            .exists(),
        "text info should exist"
    );
    assert!(
        unpack_root
            .join("golang.org!x!text/@v/v0.3.7.mod")
            .exists(),
        "text mod should exist"
    );
    assert!(
        !unpack_root
            .join("github.com!stretchr!testify/@v/v1.8.4.info")
            .exists(),
        "testify should NOT exist in staged unpack"
    );

    // ── 5. Unpack full archive with staged filter ─────────────────
    let unpack_filtered = tmp.path().join("unpacked-filtered");
    println!(
        "\nStep 5: Unpacking FULL archive with staged filter → {}",
        unpack_filtered.display()
    );

    let opts = UnpackOpts {
        dry_run: false,
        staged_filter: Some(staged_filter),
    };

    let stats2 =
        unpack_modules_with_opts(&full_archive, &unpack_filtered, &opts).expect("filtered unpack failed");

    println!(
        "  Filtered unpack stats: extracted={}, skipped={}, failed={}",
        stats2.extracted, stats2.skipped, stats2.failed
    );

    // Should extract 5 (staged) and skip 2 (testify)
    assert_eq!(stats2.extracted, 5, "Should extract 5 staged files from full archive");
    assert_eq!(stats2.skipped, 2, "Should skip 2 non-staged files (testify)");
    assert_eq!(stats2.failed, 0, "Should have 0 failures");

    // Verify testify was filtered out
    assert!(
        !unpack_filtered
            .join("github.com!stretchr!testify/@v/v1.8.4.info")
            .exists(),
        "testify should NOT exist after staged-filtered unpack"
    );

    println!("\n=== Phase 4 Staged Pack/Unpack Test PASSED (◕‿◕✿) ===");
}
