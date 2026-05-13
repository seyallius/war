//! sync_integration_test.rs - Integration tests for `war go sync`.
//!
//! These tests simulate the full Phase 5 workflow:
//!   1. Build a synthetic war cache (as produced by `war go unpack`).
//!   2. Call `sync_cache` to copy files into a mock native Go module cache.
//!   3. Assert that the resulting directory tree matches the GOPROXY-protocol
//!      layout that Go tooling expects.
//!
//! Tests do **not** invoke Go tooling directly (no network, no $PATH dependency)
//! — they verify structural correctness at the filesystem level.

use std::{fs, path::Path};
use tempfile::TempDir;
use war_go::{sync_cache, SyncStats};

// -------------------------------------------- Tests --------------------------------------------

/// The entire Phase 5 airgap loop:
/// war-cache layout → sync → native cache layout
///
/// This is the "real-world validation" test described in the objective.
/// It verifies that after `war go sync`:
/// - All `.info`, `.mod`, and `.zip` files are present in the native cache.
/// - File contents are identical to the source.
/// - The directory structure is exactly what Go's GOPROXY protocol expects.
#[test]
fn test_full_airgap_sync_to_native_cache() {
    let tmp = TempDir::new().unwrap();
    let war_cache = tmp.path().join("war_cache");
    let native_cache = tmp.path().join("gomodcache");

    // Simulate what `war go unpack cache.zip` writes to ~/.war/cache/go:
    scaffold_war_cache(
        &war_cache,
        &[
            // gin v1.9.1 — three canonical GOPROXY files
            (
                "github.com!gin-gonic!gin/@v/v1.9.1.info",
                br#"{"Version":"v1.9.1","Time":"2025-01-01T00:00:00Z"}"#,
            ),
            (
                "github.com!gin-gonic!gin/@v/v1.9.1.mod",
                b"module github.com/gin-gonic/gin\n\ngo 1.20\n",
            ),
            (
                "github.com!gin-gonic!gin/@v/v1.9.1.zip",
                b"PK\x03\x04mock-zip-bytes",
            ),
            // golang.org/x/text v0.3.7
            (
                "golang.org!x!text/@v/v0.3.7.info",
                br#"{"Version":"v0.3.7","Time":"2025-01-01T00:00:00Z"}"#,
            ),
            (
                "golang.org!x!text/@v/v0.3.7.mod",
                b"module golang.org/x/text\n\ngo 1.17\n",
            ),
            // list file — Go writes a `list` file that enumerates known versions
            ("github.com!gin-gonic!gin/@v/list", b"v1.9.1\n"),
        ],
    );

    // ── Phase 5 action ──────────────────────────────────────────────────────
    let result = sync_cache(&war_cache, &native_cache)
        .expect("sync_cache must succeed for the full airgap workflow");

    // ── Assert stats ────────────────────────────────────────────────────────
    assert_eq!(
        result.stats.copied, 6,
        "all 6 files must be copied on first sync"
    );
    assert_eq!(result.stats.skipped, 0);
    assert_eq!(result.stats.failed, 0);
    assert_eq!(result.stats.collisions, 0);

    // ── Assert native cache layout ──────────────────────────────────────────
    // Go tooling looks for exactly these paths when $GOPROXY=file://<native_cache>
    // or when reading from the native module cache directly.

    let gin_dir = native_cache.join("github.com!gin-gonic!gin/@v");
    assert!(gin_dir.exists(), "gin @v directory must exist");

    let info = gin_dir.join("v1.9.1.info");
    assert!(info.exists(), ".info file must be present");
    assert!(
        read_file(&info).contains("v1.9.1"),
        ".info content must contain version"
    );

    let modfile = gin_dir.join("v1.9.1.mod");
    assert!(modfile.exists(), ".mod file must be present");
    assert!(
        read_file(&modfile).contains("gin-gonic/gin"),
        ".mod content must contain module path"
    );

    let zipfile = gin_dir.join("v1.9.1.zip");
    assert!(zipfile.exists(), ".zip file must be present");

    let text_dir = native_cache.join("golang.org!x!text/@v");
    assert!(text_dir.exists(), "text @v directory must exist");
    assert!(text_dir.join("v0.3.7.info").exists());
    assert!(text_dir.join("v0.3.7.mod").exists());

    // ── Idempotency: second sync must skip everything ──────────────────────
    let result2 = sync_cache(&war_cache, &native_cache).expect("second sync must succeed");

    assert_eq!(
        result2.stats.skipped, 6,
        "second sync must skip all 6 files (idempotent)"
    );
    assert_eq!(result2.stats.copied, 0);
    assert_eq!(result2.stats.collisions, 0);
    assert_eq!(result2.stats.failed, 0);
}

/// Partial sync: war cache has more modules than what was previously synced.
///
/// Simulates the common workflow where the user unpacks more modules and
/// re-runs `war go sync` — only the new arrivals should be copied.
#[test]
fn test_incremental_sync_copies_only_new_files() {
    let tmp = TempDir::new().unwrap();
    let war_cache = tmp.path().join("war_cache");
    let native_cache = tmp.path().join("gomodcache");

    // First batch: gin only.
    scaffold_war_cache(
        &war_cache,
        &[(
            "github.com!gin-gonic!gin/@v/v1.9.1.mod",
            b"module github.com/gin-gonic/gin\n",
        )],
    );

    let r1 = sync_cache(&war_cache, &native_cache).unwrap();
    assert_eq!(r1.stats.copied, 1);

    // Second batch: add golang.org/x/text.
    scaffold_war_cache(
        &war_cache,
        &[(
            "golang.org!x!text/@v/v0.3.7.mod",
            b"module golang.org/x/text\n",
        )],
    );

    let r2 = sync_cache(&war_cache, &native_cache).unwrap();
    assert_eq!(r2.stats.copied, 1, "only the new file should be copied");
    assert_eq!(r2.stats.skipped, 1, "gin mod must be skipped (unchanged)");
    assert_eq!(r2.stats.collisions, 0);
}

/// Collision: a file already in the native cache was modified by the user.
/// `war go sync` must warn and refuse to overwrite.
#[test]
fn test_sync_does_not_overwrite_user_modified_files() {
    let tmp = TempDir::new().unwrap();
    let war_cache = tmp.path().join("war_cache");
    let native_cache = tmp.path().join("gomodcache");

    // We need same-size, different-content to trigger the hash-mismatch path.
    // Both contents are exactly 30 bytes.
    let war_content = b"module gin // war-synced-v1.9.1";
    let user_content = b"module gin // user-patched!!!!!";
    assert_eq!(war_content.len(), user_content.len());

    scaffold_war_cache(&war_cache, &[("gin/@v/v1.9.1.mod", war_content)]);
    // Pre-populate native cache with the "user-modified" version.
    scaffold_war_cache(&native_cache, &[("gin/@v/v1.9.1.mod", user_content)]);

    let result = sync_cache(&war_cache, &native_cache).unwrap();

    assert_eq!(result.stats.collisions, 1, "one collision must be detected");
    assert_eq!(result.stats.copied, 0, "must NOT copy over a collision");
    assert_eq!(result.warnings.len(), 1);

    // The user's file must be untouched.
    let dst = native_cache.join("gin/@v/v1.9.1.mod");
    let on_disk = fs::read(&dst).unwrap();
    assert_eq!(
        on_disk, user_content,
        "user-modified file must not be overwritten"
    );

    // The warning must record both hashes.
    let warn = &result.warnings[0];
    assert!(!warn.source_hash.is_empty());
    assert!(!warn.destination_hash.is_empty());
    assert_ne!(
        warn.source_hash, warn.destination_hash,
        "hashes must differ for a collision"
    );
}

/// Verify that a sync of an empty war cache succeeds without errors.
#[test]
fn test_sync_empty_cache_succeeds() {
    let tmp = TempDir::new().unwrap();
    let war_cache = tmp.path().join("war_cache");
    let native_cache = tmp.path().join("gomodcache");

    // Empty but existing war cache.
    fs::create_dir_all(&war_cache).unwrap();

    let result = sync_cache(&war_cache, &native_cache).unwrap();

    assert_eq!(result.stats, SyncStats::default());
    assert!(result.warnings.is_empty());
}

/// `sync_cache` must return `WarError::InvalidInput` when the war cache
/// does not exist yet (user forgot to run `war go unpack` first).
#[test]
fn test_sync_missing_src_returns_error() {
    let tmp = TempDir::new().unwrap();
    let war_cache = tmp.path().join("does_not_exist");
    let native_cache = tmp.path().join("gomodcache");

    let err = sync_cache(&war_cache, &native_cache).unwrap_err();

    let msg = err.to_string();
    assert!(
        msg.contains("does not exist"),
        "error message must mention missing source: {}",
        msg
    );
}

/// Multi-version sync: two versions of the same module coexist without
/// interfering with each other.
#[test]
fn test_sync_multiple_versions_of_same_module() {
    let tmp = TempDir::new().unwrap();
    let war_cache = tmp.path().join("war_cache");
    let native_cache = tmp.path().join("gomodcache");

    scaffold_war_cache(
        &war_cache,
        &[
            (
                "github.com!stretchr!testify/@v/v1.8.0.mod",
                b"module github.com/stretchr/testify\n\ngo 1.13\n",
            ),
            (
                "github.com!stretchr!testify/@v/v1.8.4.mod",
                b"module github.com/stretchr/testify\n\ngo 1.13\n",
            ),
            ("github.com!stretchr!testify/@v/list", b"v1.8.0\nv1.8.4\n"),
        ],
    );

    let result = sync_cache(&war_cache, &native_cache).unwrap();

    assert_eq!(result.stats.copied, 3);
    assert_eq!(result.stats.failed, 0);
    assert_eq!(result.stats.collisions, 0);

    let testify_dir = native_cache.join("github.com!stretchr!testify/@v");
    assert!(testify_dir.join("v1.8.0.mod").exists());
    assert!(testify_dir.join("v1.8.4.mod").exists());
    assert!(testify_dir.join("list").exists());
}

// -------------------------------------------- Helpers --------------------------------------------

/// Build a minimal war cache in `root` from a list of `(relative_path, content)` pairs.
///
/// Mirrors the layout produced by `war go unpack`: every module path uses
/// `!`-encoded segments (Go cache convention) and sits under an `@v/` sub-dir.
fn scaffold_war_cache(root: &Path, files: &[(&str, &[u8])]) {
    for (rel, content) in files {
        let dest = root.join(rel);
        fs::create_dir_all(dest.parent().expect("parent dir")).unwrap();
        fs::write(&dest, content).unwrap();
    }
}

/// Read the content of `path` as a `String`, panicking with a clear message
/// if the file is missing.
fn read_file(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|_| panic!("expected file at {}", path.display()))
}
