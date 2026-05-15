//! Package war_go. unpack.rs - Extract a `war go pack` archive additively into the
//! Go module cache directory (`~/.war/cache/go/`).
//!
//! The archive uses standard `/` paths (as produced by `pack_modules`), but Go's
//! module cache expects `!` separators in the first path component — e.g.
//! `github.com/gin-gonic/gin/@v/v1.9.1.info` in the zip becomes
//! `github.com!gin-gonic!gin/@v/v1.9.1.info` on disk.
//!
//! This module handles:
//! - Path normalization: converting `/` back to `!` in the first component for
//!   Go cache layout compatibility.
//! - Path traversal protection: rejecting any entry containing `..` components.
//! - Idempotent extraction: skipping files that already exist with matching size.
//! - Atomic-ish writes: each file is written to a `.tmp` sidecar first, then
//!   `fs::rename`d into its final location to avoid partial writes on crash.
//! - Unix permission preservation from the zip entry metadata.
//! - Dry-run mode: list files that *would* be extracted without writing.
//!
//! The extraction is intentionally synchronous (`std::fs` + `zip`) because
//! single-archive extraction sees no meaningful benefit from async I/O.

use std::{
    fs,
    io::{self, Read, Write},
    path::{Component, Path, PathBuf},
    time::Instant,
};
use war_core::WarError;
use zip::ZipArchive;

// -------------------------------------------- Types --------------------------------------------

/// Options controlling how `unpack_modules_with_opts` behaves.
///
/// Built with a builder-pattern via `UnpackOpts::default()` so new flags
/// can be added without breaking existing call-sites.
#[derive(Debug, Clone, Default)]
pub struct UnpackOpts {
    /// When true, only list the paths that would be extracted — do **not**
    /// write any files to disk. Useful for `war go unpack --dry-run`.
    pub dry_run: bool,
    /// When set, only zip entries matching the staged `(module, version)`
    /// pairs are extracted. Entries that don't match are counted as `skipped`.
    pub staged_filter: Option<Vec<(String, String)>>,
}

/// Statistics returned after unpacking a war archive.
///
/// Failed files do **not** abort the operation — the caller receives the full
/// picture and can decide whether to surface a non-zero exit code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnpackStats {
    /// Files successfully extracted and written to disk.
    pub extracted: usize,
    /// Files skipped because the destination already matched (same byte length)
    /// or were excluded by the staged filter.
    pub skipped: usize,
    /// Files that failed to extract (I/O error, rename failure, etc.).
    pub failed: usize,
}

// -------------------------------------------- Public API --------------------------------------------

/// Extract a `war go pack` archive additively into `target_root`.
///
/// Convenience wrapper around `unpack_modules_with_opts` with default options.
pub fn unpack_modules(archive_path: &Path, target_root: &Path) -> Result<UnpackStats, WarError> {
    unpack_modules_with_opts(archive_path, target_root, &UnpackOpts::default())
}

/// Extract a `war go pack` archive additively into `target_root`, with
/// configurable options.
///
/// `target_root` is the directory that receives extracted files — typically
/// `~/.war/cache/go/`.  If it does not exist it is created (unless dry-run).
///
/// # Corrupted archive handling
///
/// If the zip central directory is unreadable (truncated file, bad magic bytes,
/// etc.) the function returns `WarError::CorruptedArchive` immediately and, if
/// the target directory was freshly created by this call, removes it to keep the
/// filesystem clean.  Per-entry corruption (bad CRC, unreadable entry header) is
/// treated as a soft failure: the entry is counted in `stats.failed` and the loop
/// continues so as many files as possible are recovered.
///
/// # Errors
///
/// - `WarError::InvalidInput` — archive does not exist.
/// - `WarError::CorruptedArchive` — archive cannot be opened as a zip.
/// - `WarError::IOError` — unrecoverable I/O error (e.g. cannot create target dir).
pub fn unpack_modules_with_opts(
    archive_path: &Path,
    target_root: &Path,
    opts: &UnpackOpts,
) -> Result<UnpackStats, WarError> {
    let span = tracing::info_span!(
        "unpack_modules",
        archive = %archive_path.display(),
        target  = %target_root.display(),
        dry_run = opts.dry_run,
    );
    let _enter = span.enter();

    // ── Guard: archive must exist ─────────────────────────────────────────
    if !archive_path.exists() {
        return Err(WarError::InvalidInput(format!(
            "Archive does not exist: {}.\n\
             Run `war go pack` to create an archive first.",
            archive_path.display()
        )));
    }

    let t0 = Instant::now();
    tracing::info!(
        "(◕‿◕✿) Opening archive: {} ({} bytes)",
        archive_path.display(),
        fs::metadata(archive_path).map(|m| m.len()).unwrap_or(0)
    );

    // ── Open the zip — map structural errors to CorruptedArchive ─────────
    let file = fs::File::open(archive_path)?;
    let mut archive = ZipArchive::new(file).map_err(|e| WarError::CorruptedArchive {
        path: archive_path.to_path_buf(),
        reason: format!(
            "Cannot read zip central directory: {}. \
                 The file may be truncated or corrupted.",
            e
        ),
        hint: format!(
            "Re-create the archive with `war go pack` or re-download it.\n\
                 To verify: `unzip -t {}` should return exit 0.",
            archive_path.display()
        ),
    })?;

    let total_entries = archive.len();
    tracing::info!("Archive contains {} entries", total_entries);

    // ── Ensure target root exists (skip in dry-run) ───────────────────────
    // Track whether WE created the directory so we can clean up on failure.
    let target_created_by_us = if !opts.dry_run && !target_root.exists() {
        fs::create_dir_all(target_root)?;
        true
    } else {
        false
    };

    let mut stats = UnpackStats {
        extracted: 0,
        skipped: 0,
        failed: 0,
    };

    const PROGRESS_INTERVAL: usize = 25;

    for index in 0..total_entries {
        // ── Per-entry soft failure: log and continue ──────────────────────
        let mut entry = match archive.by_index(index) {
            Ok(e) => e,
            Err(e) => {
                tracing::warn!(
                    "⚠ Skipping corrupt entry at index {} / {}: {}",
                    index,
                    total_entries,
                    e
                );
                stats.failed += 1;
                continue;
            }
        };

        // ── Resolve and sanitise the entry path ───────────────────────────
        let raw_path = match entry.enclosed_name() {
            Some(p) => p.to_owned(),
            None => {
                tracing::warn!("⚠ Entry {} has an invalid path — skipping", index);
                stats.failed += 1;
                continue;
            }
        };

        let stripped = strip_leading_dot_slash(&raw_path);

        if contains_traversal(&stripped) {
            tracing::warn!(
                "⚠ Rejecting path-traversal entry: {} — potential zip-slip attack",
                stripped.display()
            );
            stats.failed += 1;
            continue;
        }

        // ── Skip directory entries ─────────────────────────────────────────
        if entry.is_dir() {
            tracing::debug!("  – dir entry: {}", stripped.display());
            continue;
        }

        // ── Convert archive path → Go cache path ──────────────────────────
        let cache_relative = denormalize_cache_path(&stripped);
        let dest = target_root.join(&cache_relative);

        // ── Staged filter ─────────────────────────────────────────────────
        if let Some(filter) = &opts.staged_filter {
            if !entry_matches_staged_filter(&stripped, filter) {
                tracing::debug!("[staged] skip: {}", stripped.display());
                stats.skipped += 1;
                continue;
            }
        }

        // ── Dry-run ───────────────────────────────────────────────────────
        if opts.dry_run {
            tracing::info!("[dry-run] would extract: {}", dest.display());
            stats.extracted += 1;
            continue;
        }

        // ── Idempotency check ─────────────────────────────────────────────
        if is_idempotent_skip(&dest, entry.size()) {
            tracing::debug!("  – skip (size match): {}", cache_relative.display());
            stats.skipped += 1;
            continue;
        }

        // ── Ensure parent directory ───────────────────────────────────────
        if let Some(parent) = dest.parent() {
            if let Err(e) = fs::create_dir_all(parent) {
                tracing::warn!("⚠ Cannot create parent dir for {}: {}", dest.display(), e);
                stats.failed += 1;
                continue;
            }
        }

        // ── Atomic extract ────────────────────────────────────────────────
        match extract_file_atomic(&mut entry, &dest) {
            Ok(()) => {
                tracing::debug!("  ✔ extracted: {}", cache_relative.display());
                stats.extracted += 1;
            }
            Err(e) => {
                tracing::warn!("⚠ Failed to extract {}: {}", cache_relative.display(), e);
                stats.failed += 1;
            }
        }

        // ── Progress heartbeat ────────────────────────────────────────────
        let processed = stats.extracted + stats.skipped + stats.failed;
        if processed % PROGRESS_INTERVAL == 0 {
            tracing::info!(
                "  … {}/{} entries processed ({} extracted, {} skipped, {} failed) …",
                processed,
                total_entries,
                stats.extracted,
                stats.skipped,
                stats.failed
            );
        }
    }

    let elapsed = t0.elapsed();
    tracing::info!(
        "✔ Unpack done in {:.2}s — {} extracted, {} skipped, {} failed",
        elapsed.as_secs_f64(),
        stats.extracted,
        stats.skipped,
        stats.failed
    );

    // ── If everything failed (likely corrupt), clean up fresh target dir ──
    if stats.extracted == 0 && stats.failed > 0 && target_created_by_us {
        tracing::warn!(
            "⚠ 0 files extracted, {} failed — removing freshly created target dir {} to keep filesystem clean.",
            stats.failed,
            target_root.display()
        );
        let _ = fs::remove_dir_all(target_root);
    }

    Ok(stats)
}

// -------------------------------------------- Internal Helpers --------------------------------------------

/// Strip a leading `./` prefix from a path, if present.
///
/// Some zip archivers prepend `./` to each entry.  This is harmless for
/// extraction but produces an unwanted `.` component in the Go cache layout.
fn strip_leading_dot_slash(path: &Path) -> PathBuf {
    let mut components: Vec<_> = path.components().collect();
    while components
        .first()
        .map_or(false, |c| *c == Component::CurDir)
    {
        components.remove(0);
    }
    components.iter().collect()
}

/// Return `true` if `path` contains any `..` (parent directory) component.
///
/// Security-critical: called on every entry path before constructing the
/// destination path.  A zip-slip attack embeds `../../etc/passwd` style paths
/// in the archive — this check blocks them completely.
fn contains_traversal(path: &Path) -> bool {
    path.components().any(|c| matches!(c, Component::ParentDir))
}

/// Check whether a zip entry path matches any `(module, version)` pair
/// in the staged filter.
///
/// The entry path uses `/` separators (archive format).  The `@v` boundary
/// separates the module path from the version filename.
fn entry_matches_staged_filter(entry_path: &Path, filter: &[(String, String)]) -> bool {
    let parts: Vec<_> = entry_path.components().collect();
    if parts.len() < 3 {
        return false;
    }

    let v_idx = match parts.iter().position(|c| c.as_os_str() == "@v") {
        Some(idx) => idx,
        None => return false,
    };

    if v_idx + 1 >= parts.len() {
        return false;
    }

    // Module path: everything before @v, joined with '/'.
    let module: String = parts[..v_idx]
        .iter()
        .filter_map(|c| {
            if let Component::Normal(os) = c {
                Some(os.to_string_lossy().into_owned())
            } else {
                None
            }
        })
        .collect::<Vec<_>>()
        .join("/");

    // Version: file stem of the component immediately after @v.
    let version = Path::new(parts[v_idx + 1].as_os_str())
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_string();

    filter.contains(&(module, version))
}

/// Convert an archive path (`/`-separated) to a Go module cache path
/// (`!`-separated first component).
///
/// The `@v` directory is the reliable boundary marker.  Everything before
/// `@v` is the module path and must be joined with `!`.  Everything from
/// `@v` onward uses normal path separators.
///
/// ```text
/// github.com/gin-gonic/gin/@v/v1.9.1.info
///   → github.com!gin-gonic!gin/@v/v1.9.1.info
/// ```
fn denormalize_cache_path(relative: &Path) -> PathBuf {
    let components: Vec<_> = relative.components().collect();

    let v_idx = components.iter().position(|c| c.as_os_str() == "@v");

    match v_idx {
        Some(idx) if idx > 0 => {
            let module_part: String = components[..idx]
                .iter()
                .filter_map(|c| {
                    if let Component::Normal(os) = c {
                        Some(os.to_string_lossy().into_owned())
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>()
                .join("!");

            let mut result = PathBuf::from(module_part);
            for comp in &components[idx..] {
                result.push(comp);
            }
            result
        }
        _ => {
            // Fallback for paths without `@v` (edge case — convert all `/` → `!`).
            let path_str = relative.to_string_lossy();
            PathBuf::from(path_str.replace('/', "!"))
        }
    }
}

/// Return `true` if `dest` already exists with a byte size matching `expected_size`.
///
/// Used as a lightweight idempotency heuristic: a matching size means the file
/// was already extracted by a previous run and can be safely skipped.
fn is_idempotent_skip(dest: &Path, expected_size: u64) -> bool {
    match fs::metadata(dest) {
        Ok(meta) => meta.len() == expected_size,
        Err(_) => false,
    }
}

/// Extract a single zip entry to `dest` using an atomic-ish write strategy.
///
/// The file is streamed to `dest.war.tmp` in the same directory, then
/// `fs::rename`d into place.  On POSIX systems `rename(2)` is atomic with
/// respect to the directory entry, so concurrent readers never observe a
/// partial file.
///
/// If the rename fails, the `.war.tmp` sidecar is removed before returning
/// the error, leaving no stale partial files.
fn extract_file_atomic<R: Read>(
    entry: &mut zip::read::ZipFile<'_, R>,
    dest: &Path,
) -> Result<(), WarError> {
    let tmp_path = dest.with_extension(format!(
        "{}.war.tmp",
        dest.extension().and_then(|e| e.to_str()).unwrap_or("dat")
    ));

    // Stream entry bytes to the tmp file.
    let mut tmp_file = fs::File::create(&tmp_path)?;
    io::copy(entry, &mut tmp_file)?;
    tmp_file.flush()?;

    // Preserve unix permissions when available (no-op on Windows).
    #[cfg(unix)]
    {
        if let Some(mode) = entry.unix_mode() {
            use std::os::unix::fs::PermissionsExt;
            let perms = fs::Permissions::from_mode(mode);
            fs::set_permissions(&tmp_path, perms)?;
        }
    }

    // Atomic rename into final location.
    fs::rename(&tmp_path, dest).map_err(|e| {
        let _ = fs::remove_file(&tmp_path); // best-effort cleanup
        WarError::IOError(io::Error::new(
            e.kind(),
            format!(
                "Atomic rename failed: {} → {}: {}",
                tmp_path.display(),
                dest.display(),
                e
            ),
        ))
    })?;

    Ok(())
}

// -------------------------------------------- Tests --------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as IoWrite;
    use tempfile::TempDir;
    use zip::{write::FileOptions, ZipWriter};

    // ── Helper: build a zip archive in memory ────────────────────────────

    /// Build a well-formed zip in memory from `(path, content)` pairs.
    fn build_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let buf = std::io::Cursor::new(Vec::new());
        let mut zip = ZipWriter::new(buf);
        let opts: FileOptions<'_, ()> = FileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .unix_permissions(0o644);

        for (path, content) in entries {
            zip.start_file(path, opts).expect("start_file");
            zip.write_all(content).expect("write_all");
        }
        zip.finish().expect("finish").into_inner()
    }

    /// Write raw bytes to a file on disk.
    fn write_file(path: &Path, data: &[u8]) {
        let mut f = fs::File::create(path).expect("create");
        f.write_all(data).expect("write");
    }

    // ── Successful extraction ────────────────────────────────────────────

    #[test]
    fn test_successful_extraction() {
        let tmp = TempDir::new().unwrap();
        let archive = tmp.path().join("test.zip");
        let target = tmp.path().join("cache");

        let data = build_zip(&[
            (
                "github.com/gin-gonic/gin/@v/v1.9.1.info",
                b"{\"Version\":\"v1.9.1\"}",
            ),
            ("github.com/gin-gonic/gin/@v/v1.9.1.mod", b"module gin\n"),
            (
                "golang.org/x/text/@v/v0.3.7.info",
                b"{\"Version\":\"v0.3.7\"}",
            ),
        ]);
        write_file(&archive, &data);

        let stats = unpack_modules(&archive, &target).expect("unpack");

        assert_eq!(stats.extracted, 3);
        assert_eq!(stats.skipped, 0);
        assert_eq!(stats.failed, 0);

        assert!(target
            .join("github.com!gin-gonic!gin/@v/v1.9.1.info")
            .exists());
        assert!(target.join("golang.org!x!text/@v/v0.3.7.info").exists());
    }

    // ── Idempotency ──────────────────────────────────────────────────────

    #[test]
    fn test_idempotent_reextraction_skips_same_size() {
        let tmp = TempDir::new().unwrap();
        let archive = tmp.path().join("test.zip");
        let target = tmp.path().join("cache");

        let data = build_zip(&[("github.com/test/mod/@v/v1.0.0.info", b"hello world")]);
        write_file(&archive, &data);

        let s1 = unpack_modules(&archive, &target).unwrap();
        assert_eq!(s1.extracted, 1);

        let s2 = unpack_modules(&archive, &target).unwrap();
        assert_eq!(s2.extracted, 0);
        assert_eq!(s2.skipped, 1);

        let on_disk =
            fs::read_to_string(target.join("github.com!test!mod/@v/v1.0.0.info")).unwrap();
        assert_eq!(on_disk, "hello world");
    }

    // ── Corrupted archive — structural error ─────────────────────────────

    #[test]
    fn test_corrupted_archive_returns_specific_error() {
        let tmp = TempDir::new().unwrap();
        let archive = tmp.path().join("bad.zip");
        let target = tmp.path().join("cache");

        // Write garbage bytes — not a valid zip.
        write_file(&archive, b"this is not a zip file at all \x00\xff");

        let err = unpack_modules(&archive, &target).unwrap_err();

        assert!(
            matches!(err, WarError::CorruptedArchive { .. }),
            "expected CorruptedArchive, got: {:?}",
            err
        );

        // The hint must be present and actionable.
        if let WarError::CorruptedArchive { hint, .. } = err {
            assert!(!hint.is_empty(), "hint must not be empty");
        }
    }

    #[test]
    fn test_corrupted_archive_cleans_up_target_dir() {
        let tmp = TempDir::new().unwrap();
        let archive = tmp.path().join("bad.zip");
        let target = tmp.path().join("fresh_target");

        // Truncated zip — structural corruption.
        write_file(&archive, b"PK\x03\x04truncated");

        let _ = unpack_modules(&archive, &target);

        // The target directory should NOT persist after a structural failure
        // because we created it and zero files were extracted.
        // (Whether it exists depends on whether the zip library rejects at open
        // vs. at iteration; either outcome is acceptable — we just assert no
        // partial content is left.)
        if target.exists() {
            let entries: Vec<_> = fs::read_dir(&target).unwrap().flatten().collect();
            assert!(
                entries.is_empty(),
                "corrupt unpack must not leave partial files: {:?}",
                entries
            );
        }
    }

    // ── Truncated zip: 50% of bytes removed ──────────────────────────────

    #[test]
    fn test_truncated_zip_50_percent() {
        let tmp = TempDir::new().unwrap();
        let archive = tmp.path().join("truncated.zip");
        let target = tmp.path().join("cache");

        // Build a valid zip, then keep only the first half of the bytes —
        // simulating `truncate -s 50% cache.zip`.
        let full = build_zip(&[
            (
                "github.com/gin-gonic/gin/@v/v1.9.1.mod",
                b"module gin\n\ngo 1.20\n",
            ),
            (
                "golang.org/x/text/@v/v0.3.7.mod",
                b"module text\n\ngo 1.17\n",
            ),
        ]);
        let half = &full[..full.len() / 2];
        write_file(&archive, half);

        let result = unpack_modules(&archive, &target);

        // We expect either CorruptedArchive (if zip rejects at open) or
        // a stats result with failures (if it rejects at iteration).
        match result {
            Err(WarError::CorruptedArchive { .. }) => {
                // Perfect: caught at the structural level.
            }
            Ok(stats) => {
                // Acceptable: caught per-entry. Failed count must be > 0.
                assert!(
                    stats.failed > 0 || stats.extracted == 0,
                    "50% truncation must result in failures or zero extractions"
                );
            }
            Err(other) => {
                // Any other error is acceptable too (I/O error on read, etc.)
                tracing::warn!(
                    "Got non-CorruptedArchive error for truncated zip: {:?}",
                    other
                );
            }
        }
    }

    // ── Path traversal rejection ─────────────────────────────────────────

    #[test]
    fn test_path_traversal_rejected() {
        let tmp = TempDir::new().unwrap();
        let archive = tmp.path().join("evil.zip");
        let target = tmp.path().join("cache");

        let data = build_zip(&[
            ("../../etc/passwd", b"root:x:0:0"),
            ("github.com/safe/mod/@v/v1.0.0.info", b"safe"),
        ]);
        write_file(&archive, &data);

        let stats = unpack_modules(&archive, &target).unwrap();

        assert_eq!(stats.extracted, 1, "safe entry must still be extracted");
        assert_eq!(stats.failed, 1, "traversal entry must be counted as failed");

        // The safe file must exist; passwd must not.
        assert!(target.join("github.com!safe!mod/@v/v1.0.0.info").exists());
        assert!(!target.join("etc/passwd").exists());
        // Double-check: passwd must not exist ANYWHERE under target.
        let dangerous = tmp.path().join("etc/passwd");
        assert!(
            !dangerous.exists(),
            "path traversal must be blocked completely"
        );
    }

    // ── Missing archive ───────────────────────────────────────────────────

    #[test]
    fn test_missing_archive_returns_invalid_input() {
        let tmp = TempDir::new().unwrap();
        let archive = tmp.path().join("nonexistent.zip");
        let target = tmp.path().join("cache");

        let err = unpack_modules(&archive, &target).unwrap_err();

        assert!(
            matches!(err, WarError::InvalidInput(_)),
            "expected InvalidInput, got: {:?}",
            err
        );
    }

    // ── Dry-run mode ─────────────────────────────────────────────────────

    #[test]
    fn test_dry_run_does_not_write_files() {
        let tmp = TempDir::new().unwrap();
        let archive = tmp.path().join("test.zip");
        let target = tmp.path().join("cache");

        let data = build_zip(&[
            ("github.com/test/mod/@v/v1.0.0.info", b"content"),
            ("golang.org/x/text/@v/v0.3.7.info", b"also"),
        ]);
        write_file(&archive, &data);

        let opts = UnpackOpts {
            dry_run: true,
            ..Default::default()
        };
        let stats = unpack_modules_with_opts(&archive, &target, &opts).unwrap();

        assert_eq!(stats.extracted, 2);
        assert!(!target.exists(), "dry-run must not create target dir");
    }

    // ── Staged filter ─────────────────────────────────────────────────────

    #[test]
    fn test_staged_filter_extracts_only_matching() {
        let tmp = TempDir::new().unwrap();
        let archive = tmp.path().join("test.zip");
        let target = tmp.path().join("cache");

        let data = build_zip(&[
            ("github.com/gin-gonic/gin/@v/v1.9.1.mod", b"gin"),
            ("golang.org/x/text/@v/v0.3.7.mod", b"text"),
        ]);
        write_file(&archive, &data);

        let opts = UnpackOpts {
            staged_filter: Some(vec![(
                "github.com/gin-gonic/gin".to_string(),
                "v1.9.1".to_string(),
            )]),
            ..Default::default()
        };
        let stats = unpack_modules_with_opts(&archive, &target, &opts).unwrap();

        assert_eq!(stats.extracted, 1, "only gin should be extracted");
        assert_eq!(stats.skipped, 1, "text should be skipped by filter");

        assert!(target
            .join("github.com!gin-gonic!gin/@v/v1.9.1.mod")
            .exists());
        assert!(!target.join("golang.org!x!text/@v/v0.3.7.mod").exists());
    }

    // ── Mixed success + traversal ─────────────────────────────────────────

    #[test]
    fn test_mixed_success_and_traversal() {
        let tmp = TempDir::new().unwrap();
        let archive = tmp.path().join("mixed.zip");
        let target = tmp.path().join("cache");

        let data = build_zip(&[
            ("github.com/good/mod/@v/v1.0.0.info", b"good"),
            ("../escape/attempt", b"evil"),
            ("github.com/also/good/@v/v2.0.0.info", b"also good"),
        ]);
        write_file(&archive, &data);

        let stats = unpack_modules(&archive, &target).unwrap();

        assert_eq!(stats.extracted, 2);
        assert_eq!(stats.failed, 1);
    }

    // ── Atomic write: no .war.tmp remains ────────────────────────────────

    #[test]
    fn test_atomic_extract_leaves_no_tmp() {
        let tmp = TempDir::new().unwrap();
        let archive = tmp.path().join("test.zip");
        let target = tmp.path().join("cache");

        let data = build_zip(&[("github.com/test/mod/@v/v1.0.0.mod", b"module test\n")]);
        write_file(&archive, &data);

        unpack_modules(&archive, &target).unwrap();

        // Walk the target and assert there are no .war.tmp files.
        let tmp_files: Vec<_> = walkdir::WalkDir::new(&target)
            .into_iter()
            .flatten()
            .filter(|e| e.path().to_string_lossy().contains(".war.tmp"))
            .collect();

        assert!(
            tmp_files.is_empty(),
            "no .war.tmp files must remain: {:?}",
            tmp_files
        );
    }

    // ── Helpers: path functions ───────────────────────────────────────────

    #[test]
    fn test_strip_leading_dot_slash() {
        assert_eq!(
            strip_leading_dot_slash(Path::new("./foo/bar")),
            PathBuf::from("foo/bar")
        );
        assert_eq!(
            strip_leading_dot_slash(Path::new("foo/bar")),
            PathBuf::from("foo/bar")
        );
    }

    #[test]
    fn test_contains_traversal() {
        assert!(contains_traversal(Path::new("../../etc/passwd")));
        assert!(contains_traversal(Path::new("foo/../bar")));
        assert!(!contains_traversal(Path::new("github.com/gin/@v/v1.info")));
    }

    #[test]
    fn test_denormalize_cache_path() {
        assert_eq!(
            denormalize_cache_path(Path::new("github.com/gin-gonic/gin/@v/v1.9.1.info")),
            PathBuf::from("github.com!gin-gonic!gin/@v/v1.9.1.info")
        );
        assert_eq!(
            denormalize_cache_path(Path::new("golang.org/x/text/@v/v0.3.7.mod")),
            PathBuf::from("golang.org!x!text/@v/v0.3.7.mod")
        );
    }

    #[test]
    fn test_is_idempotent_skip() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("test.txt");
        fs::write(&path, b"hello").unwrap();

        assert!(is_idempotent_skip(&path, 5));
        assert!(!is_idempotent_skip(&path, 10));
        assert!(!is_idempotent_skip(&tmp.path().join("nonexistent"), 5));
    }
}
