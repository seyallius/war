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
};
use war_core::WarError;
use zip::ZipArchive;

// ----------------------- Public API -----------------------

/// Options controlling how `unpack_modules` behaves.
///
/// Built with a builder-pattern via `UnpackOpts::default()` so new flags
/// can be added without breaking existing call-sites.
#[derive(Debug, Clone, Default)]
pub struct UnpackOpts {
    /// When true, only list the paths that would be extracted — do **not**
    /// write any files to disk.  Useful for `war go unpack --dry-run`.
    pub dry_run: bool,
    /// When set, only zip entries matching the staged `(module, version)`
    /// pairs are extracted.  Entries that don't match are silently skipped
    /// (counted as `skipped` in `UnpackStats`).  Populated by the CLI
    /// when `--staged` is passed.
    pub staged_filter: Option<Vec<(String, String)>>,
}

/// Statistics returned after unpacking a war archive.
///
/// Provides a breakdown of how many files were extracted, how many were
/// skipped (already present with matching content), and how many failed
/// during the extraction process. Failed files do **not** abort the entire
/// operation — the caller receives the full picture and can decide how to
/// handle individual failures.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnpackStats {
    /// Number of files successfully extracted and written to disk.
    pub extracted: usize,
    /// Number of files skipped because the destination already existed
    /// with the same byte length (idempotent re-extraction).
    pub skipped: usize,
    /// Number of files that failed to extract (I/O error, permission
    /// denied, etc.). The operation continues past failures so that
    /// as many files as possible are recovered.
    pub failed: usize,
}

/// Extract a `war go pack` archive additively into `target_root` (convenience
/// wrapper with default options).
///
/// Equivalent to `unpack_modules_with_opts(archive_path, target_root,
/// &UnpackOpts::default())`.
pub fn unpack_modules(archive_path: &Path, target_root: &Path) -> Result<UnpackStats, WarError> {
    unpack_modules_with_opts(archive_path, target_root, &UnpackOpts::default())
}

/// Extract a `war go pack` archive additively into `target_root`, with
/// configurable options.
///
/// `target_root` is the directory that will receive the extracted files —
/// typically `~/.war/cache/go/` or a test-specific directory. If it does not
/// exist, it will be created (unless `dry_run` is set).
///
/// Each zip entry's path is normalized from the archive format (`/`-separated
/// module paths) to the Go module cache format (`!`-separated first component).
/// For example, `github.com/gin-gonic/gin/@v/v1.9.1.info` becomes
/// `github.com!gin-gonic!gin/@v/v1.9.1.info` on disk.
///
/// # Dry-run mode
///
/// When `opts.dry_run` is true, the function reads the archive and prints
/// each path that *would* be extracted via `tracing::info!`, but does **not**
/// create directories or write files.  The returned `UnpackStats` reflects
/// what would happen (extracted = would-be-extracted count).
///
/// # Errors
///
/// Returns `WarError::InvalidInput` if the archive does not exist.
/// Returns `WarError::IOError` if the archive cannot be opened.
/// Individual file extraction failures are counted in `UnpackStats::failed`
/// rather than aborting the entire operation.
///
/// # Security
///
/// Any zip entry whose path contains `..` components is rejected to prevent
/// directory traversal attacks (e.g. `../../etc/passwd`).
pub fn unpack_modules_with_opts(
    archive_path: &Path,
    target_root: &Path,
    opts: &UnpackOpts,
) -> Result<UnpackStats, WarError> {
    if !archive_path.exists() {
        return Err(WarError::InvalidInput(format!(
            "Archive does not exist: {}",
            archive_path.display()
        )));
    }

    tracing::info!("Opening archive: {}", archive_path.display());

    let file = fs::File::open(archive_path)?;
    let mut archive = ZipArchive::new(file).map_err(|e| {
        WarError::IOError(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "Failed to open zip archive at {}: {}",
                archive_path.display(),
                e
            ),
        ))
    })?;

    tracing::info!("Archive contains {} entries", archive.len());

    // Ensure the target root directory exists before extracting anything
    // (skip in dry-run to avoid side-effects).
    if !opts.dry_run {
        fs::create_dir_all(target_root)?;
    }

    let mut stats = UnpackStats {
        extracted: 0,
        skipped: 0,
        failed: 0,
    };

    for index in 0..archive.len() {
        let mut entry = match archive.by_index(index) {
            Ok(entry) => entry,
            Err(e) => {
                tracing::warn!("Failed to read zip entry at index {}: {}", index, e);
                stats.failed += 1;
                continue;
            }
        };

        let raw_path = match entry.enclosed_name() {
            Some(path) => path.to_owned(),
            None => {
                tracing::warn!(
                    "Skipping zip entry at index {}: path is not valid UTF-8 or is empty",
                    index
                );
                stats.failed += 1;
                continue;
            }
        };

        // Strip leading "./" that some zip tools prepend.
        let stripped = strip_leading_dot_slash(&raw_path);

        // Reject any path with `..` components — directory traversal defense.
        if contains_traversal(&stripped) {
            tracing::warn!(
                "Rejecting zip entry with path traversal: {}",
                stripped.display()
            );
            stats.failed += 1;
            continue;
        }

        // Skip directory entries — we create directories on-demand when
        // extracting files, and Go's cache layout doesn't require empty
        // directory entries to function.
        if entry.is_dir() {
            tracing::debug!("Skipping directory entry: {}", stripped.display());
            continue;
        }

        // Convert archive path (`/`-separated) to Go cache path (`!`-separated
        // first component). This is the inverse of `pack_modules`'s
        // `normalize_cache_path`.
        let cache_relative = denormalize_cache_path(&stripped);
        let dest = target_root.join(&cache_relative);

        // --- Staged filter: skip entries that don't match the staged list ---
        if let Some(filter) = &opts.staged_filter {
            if !entry_matches_staged_filter(&stripped, filter) {
                tracing::debug!(
                    "[staged] Skipping non-staged entry: {}",
                    stripped.display()
                );
                stats.skipped += 1;
                continue;
            }
        }

        // --- Dry-run: just print and count, don't write ---
        if opts.dry_run {
            tracing::info!("[dry-run] would extract: {}", dest.display());
            stats.extracted += 1;
            continue;
        }

        // Idempotency check: if the file already exists and has the same
        // byte length, skip extraction. This allows re-running unpack on
        // an already-populated cache without overwriting existing files.
        if is_idempotent_skip(&dest, entry.size()) {
            tracing::info!("Skipping existing file (size match): {}", dest.display());
            stats.skipped += 1;
            continue;
        }

        // Ensure the parent directory exists before writing the file.
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)?;
        }

        // Extract the file using atomic-ish write: write to a `.tmp` sidecar
        // in the same directory, then `fs::rename` into the final location.
        // This prevents partial writes from being visible if the process
        // crashes mid-extraction.
        match extract_file_atomic(&mut entry, &dest) {
            Ok(()) => {
                tracing::info!("Extracted: {}", dest.display());
                stats.extracted += 1;
            }
            Err(e) => {
                tracing::warn!("Failed to extract {}: {}", dest.display(), e);
                stats.failed += 1;
            }
        }
    }

    tracing::info!(
        "Unpack complete: {} extracted, {} skipped, {} failed",
        stats.extracted,
        stats.skipped,
        stats.failed
    );

    Ok(stats)
}

// ----------------------- Internal Helpers -----------------------

/// Strip a leading `./` prefix from a path, if present.
///
/// Some zip archivers (including the `zip` crate when given relative paths)
/// prepend `./` to each entry. This is harmless for extraction but produces
/// an unwanted `.` component in the Go cache layout, so we strip it.
fn strip_leading_dot_slash(path: &Path) -> PathBuf {
    let mut components: Vec<_> = path.components().collect();
    // Remove leading `.` component if followed by a normal component.
    // This handles both `./foo/bar` and `./` prefix cases.
    while components
        .first()
        .map_or(false, |c| *c == Component::CurDir)
    {
        components.remove(0);
    }
    components.iter().collect()
}

/// Check whether a path contains `..` (parent directory) components,
/// indicating a potential directory traversal attack.
///
/// This is a security-critical function. Any zip entry whose normalized
/// path contains a `Component::ParentDir` is rejected outright.
fn contains_traversal(path: &Path) -> bool {
    path.components()
        .any(|c| matches!(c, Component::ParentDir))
}

/// Check whether a zip entry path matches any `(module, version)` pair
/// in the staged filter list.
///
/// The entry path uses `/` separators (archive format).  The `@v`
/// boundary is used to split the module path from the version, mirroring
/// the logic in `pack::matches_filter`.
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

    // Module path is everything before @v, joined with `/`.
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

    // Version is the file stem of the component after @v.
    let version = Path::new(parts[v_idx + 1].as_os_str())
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_string();

    filter.contains(&(module, version))
}

/// Convert a zip entry path from the archive format (using `/` separators
/// in the module path) back to the Go module cache format (using `!`
/// separators within the module path portion).
///
/// This is the inverse of `pack_modules`' `normalize_cache_path`. The
/// archive stores paths like `github.com/gin-gonic/gin/@v/v1.9.1.info`
/// (using `/` for the module path), but Go's module cache on disk uses
/// `github.com!gin-gonic!gin/@v/v1.9.1.info` (using `!` within the
/// module path so it forms a single directory name).
///
/// # Algorithm
///
/// The `@v` directory is the reliable boundary marker in Go's cache layout:
/// everything **before** `@v` belongs to the module path and must be
/// re-joined with `!`; everything from `@v` onward stays as-is.
///
/// # Examples
///
/// ```text
/// github.com/gin-gonic/gin/@v/v1.9.1.info
///   → github.com!gin-gonic!gin/@v/v1.9.1.info
///
/// golang.org/x/text/@v/v0.3.7.info
///   → golang.org!x!text/@v/v0.3.7.info
/// ```
fn denormalize_cache_path(relative: &Path) -> PathBuf {
    let components: Vec<_> = relative.components().collect();

    // Find the `@v` boundary component. Everything before it is part of
    // the module path and should be joined with `!`.
    let v_idx = components.iter().position(|c| c.as_os_str() == "@v");

    match v_idx {
        Some(idx) if idx > 0 => {
            // Join all components before `@v` with `!` to form a single
            // directory name (Go cache convention).
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
            // Append `@v` and everything after it using normal path separators.
            for comp in &components[idx..] {
                result.push(comp);
            }
            result
        }
        // If no `@v` found, or it's at position 0, fall back to replacing
        // `/` with `!` in the string representation. This handles edge
        // cases like `github.com/gin-gonic/gin` without `@v` (shouldn't
        // normally occur in a well-formed archive but we handle it).
        _ => {
            let path_str = relative.to_string_lossy();
            PathBuf::from(path_str.replace('/', "!"))
        }
    }
}

/// Check whether the destination file already exists with the same byte
/// length as the zip entry. If so, the extraction can be safely skipped
/// (idempotent behavior).
///
/// This uses file size as a quick heuristic. A more thorough check would
/// compare content hashes, but that would require reading the entire file
/// from disk — the size check is a pragmatic tradeoff for the common case
/// where re-running unpack on an already-correct cache should be a no-op.
fn is_idempotent_skip(dest: &Path, expected_size: u64) -> bool {
    match fs::metadata(dest) {
        Ok(metadata) => metadata.len() == expected_size,
        Err(_) => false,
    }
}

/// Extract a single zip entry to `dest` using an atomic-ish write strategy.
///
/// The file is first written to a `.tmp` sidecar in the same directory as
/// `dest`, then `fs::rename`d into place. Because `rename` on the same
/// filesystem is atomic on POSIX systems, this prevents partial writes from
/// being visible to concurrent readers.
///
/// If a stale `.tmp` file already exists from a previous failed extraction,
/// it is overwritten. This is safe because the `.tmp` file is never the
/// final artifact — only the rename'd file is.
fn extract_file_atomic<R: Read>(
    entry: &mut zip::read::ZipFile<'_, R>,
    dest: &Path,
) -> Result<(), WarError> {
    let tmp_path = dest.with_extension(format!(
        "{}.tmp",
        dest.extension()
            .and_then(|e| e.to_str())
            .unwrap_or("dat")
    ));

    // Create and write the temporary file.
    let mut tmp_file = fs::File::create(&tmp_path)?;
    io::copy(entry, &mut tmp_file)?;
    tmp_file.flush()?;

    // Preserve unix permissions from the zip entry, if available.
    // On non-unix platforms this is a no-op.
    #[cfg(unix)]
    {
        if let Some(mode) = entry.unix_mode() {
            use std::os::unix::fs::PermissionsExt;
            let permissions = fs::Permissions::from_mode(mode);
            fs::set_permissions(&tmp_path, permissions)?;
        }
    }

    // Atomic rename into final location.
    fs::rename(&tmp_path, dest)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Write as IoWrite};
    use tempfile::TempDir;
    use zip::{write::FileOptions, ZipWriter};

    // ----------------------- Helper: create a synthetic zip archive -----------------------

    /// Build a zip archive in memory from a list of (path, content) pairs.
    /// Returns the raw bytes of the zip file.
    fn build_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let buf = Cursor::new(Vec::new());
        let mut zip = ZipWriter::new(buf);
        let options: FileOptions<'_, ()> = FileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .unix_permissions(0o644);

        for (path, content) in entries {
            zip.start_file(path, options).expect("start_file failed");
            zip.write_all(content).expect("write_all failed");
        }

        let buf = zip.finish().expect("zip finish failed");
        buf.into_inner()
    }

    /// Write a zip archive to a file on disk.
    fn write_zip_to_file(path: &Path, data: &[u8]) {
        let mut f = fs::File::create(path).expect("create file failed");
        f.write_all(data).expect("write_all failed");
    }

    // ----------------------- Test: successful extraction -----------------------

    #[test]
    fn test_successful_extraction() {
        let tmp = TempDir::new().expect("tempdir failed");
        let archive_path = tmp.path().join("test.zip");
        let target = tmp.path().join("cache");

        let zip_data = build_zip(&[
            (
                "github.com/gin-gonic/gin/@v/v1.9.1.info",
                b"{\"Version\":\"v1.9.1\"}",
            ),
            (
                "github.com/gin-gonic/gin/@v/v1.9.1.mod",
                b"module github.com/gin-gonic/gin\n",
            ),
            (
                "golang.org/x/text/@v/v0.3.7.info",
                b"{\"Version\":\"v0.3.7\"}",
            ),
        ]);
        write_zip_to_file(&archive_path, &zip_data);

        let stats = unpack_modules(&archive_path, &target).expect("unpack failed");

        assert_eq!(stats.extracted, 3);
        assert_eq!(stats.skipped, 0);
        assert_eq!(stats.failed, 0);

        // Verify Go cache layout: first component uses `!` separators.
        let info_path = target.join("github.com!gin-gonic!gin/@v/v1.9.1.info");
        assert!(info_path.exists(), "Expected file: {}", info_path.display());
        let content = fs::read_to_string(&info_path).expect("read failed");
        assert_eq!(content, "{\"Version\":\"v1.9.1\"}");

        let mod_path = target.join("github.com!gin-gonic!gin/@v/v1.9.1.mod");
        assert!(mod_path.exists(), "Expected file: {}", mod_path.display());

        let text_info = target.join("golang.org!x!text/@v/v0.3.7.info");
        assert!(text_info.exists(), "Expected file: {}", text_info.display());
    }

    // ----------------------- Test: duplicate skip (idempotency) -----------------------

    #[test]
    fn test_idempotent_skip() {
        let tmp = TempDir::new().expect("tempdir failed");
        let archive_path = tmp.path().join("test.zip");
        let target = tmp.path().join("cache");

        let content = b"hello world";
        let zip_data = build_zip(&[("github.com/test/mod/@v/v1.0.0.info", content)]);
        write_zip_to_file(&archive_path, &zip_data);

        // First extraction: should extract.
        let stats1 = unpack_modules(&archive_path, &target).expect("unpack 1 failed");
        assert_eq!(stats1.extracted, 1);
        assert_eq!(stats1.skipped, 0);

        // Second extraction: should skip (same file, same size).
        let stats2 = unpack_modules(&archive_path, &target).expect("unpack 2 failed");
        assert_eq!(stats2.extracted, 0);
        assert_eq!(stats2.skipped, 1);

        // Content should remain unchanged.
        let file_path = target.join("github.com!test!mod/@v/v1.0.0.info");
        let on_disk = fs::read_to_string(&file_path).expect("read failed");
        assert_eq!(on_disk, "hello world");
    }

    // ----------------------- Test: traversal rejection -----------------------

    #[test]
    fn test_traversal_rejection() {
        let tmp = TempDir::new().expect("tempdir failed");
        let archive_path = tmp.path().join("evil.zip");
        let target = tmp.path().join("cache");

        let zip_data = build_zip(&[
            ("../../etc/passwd", b"root:x:0:0:root:/root:/bin/bash\n"),
            ("github.com/test/mod/@v/v1.0.0.info", b"safe content"),
        ]);
        write_zip_to_file(&archive_path, &zip_data);

        let stats = unpack_modules(&archive_path, &target).expect("unpack failed");

        // The traversal entry should be rejected (failed), the safe entry extracted.
        assert_eq!(stats.extracted, 1);
        assert_eq!(stats.failed, 1);

        // Safe entry should exist.
        let safe_path = target.join("github.com!test!mod/@v/v1.0.0.info");
        assert!(safe_path.exists());
    }

    // ----------------------- Test: missing archive -----------------------

    #[test]
    fn test_missing_archive() {
        let tmp = TempDir::new().expect("tempdir failed");
        let archive_path = tmp.path().join("nonexistent.zip");
        let target = tmp.path().join("cache");

        let result = unpack_modules(&archive_path, &target);

        assert!(result.is_err());
        match result.unwrap_err() {
            WarError::InvalidInput(msg) => {
                assert!(
                    msg.contains("Archive does not exist"),
                    "Unexpected error message: {}",
                    msg
                );
            }
            other => panic!("Expected InvalidInput, got: {:?}", other),
        }
    }

    // ----------------------- Test: dry-run mode -----------------------

    #[test]
    fn test_dry_run_does_not_write_files() {
        let tmp = TempDir::new().expect("tempdir failed");
        let archive_path = tmp.path().join("test.zip");
        let target = tmp.path().join("cache");

        let zip_data = build_zip(&[
            ("github.com/test/mod/@v/v1.0.0.info", b"dry-run content"),
            ("golang.org/x/text/@v/v0.3.7.info", b"also dry"),
        ]);
        write_zip_to_file(&archive_path, &zip_data);

        let opts = UnpackOpts { dry_run: true, ..Default::default() };
        let stats = unpack_modules_with_opts(&archive_path, &target, &opts).expect("unpack failed");

        // In dry-run mode, files are counted as "would-be extracted" but
        // nothing should actually exist on disk.
        assert_eq!(stats.extracted, 2);
        assert_eq!(stats.skipped, 0);
        assert_eq!(stats.failed, 0);

        // Target directory should NOT have been created.
        assert!(
            !target.exists(),
            "dry-run should not create the target directory"
        );
    }

    // ----------------------- Test: strip_leading_dot_slash -----------------------

    #[test]
    fn test_strip_leading_dot_slash() {
        assert_eq!(
            strip_leading_dot_slash(Path::new("./github.com/test/@v/v1.0.0.info")),
            PathBuf::from("github.com/test/@v/v1.0.0.info")
        );
        assert_eq!(
            strip_leading_dot_slash(Path::new("github.com/test/@v/v1.0.0.info")),
            PathBuf::from("github.com/test/@v/v1.0.0.info")
        );
        assert_eq!(
            strip_leading_dot_slash(Path::new("././nested/./path")),
            PathBuf::from("nested/./path")
        );
    }

    // ----------------------- Test: contains_traversal -----------------------

    #[test]
    fn test_contains_traversal() {
        assert!(contains_traversal(Path::new("../../etc/passwd")));
        assert!(contains_traversal(Path::new("foo/../bar")));
        assert!(!contains_traversal(Path::new(
            "github.com/test/@v/v1.0.0.info"
        )));
        assert!(!contains_traversal(Path::new("simple/path")));
    }

    // ----------------------- Test: denormalize_cache_path -----------------------

    #[test]
    fn test_denormalize_cache_path_basic() {
        let input = Path::new("github.com/gin-gonic/gin/@v/v1.9.1.info");
        let expected = PathBuf::from("github.com!gin-gonic!gin/@v/v1.9.1.info");
        assert_eq!(denormalize_cache_path(input), expected);
    }

    #[test]
    fn test_denormalize_cache_path_simple_domain() {
        let input = Path::new("golang.org/x/text/@v/v0.3.7.info");
        let expected = PathBuf::from("golang.org!x!text/@v/v0.3.7.info");
        assert_eq!(denormalize_cache_path(input), expected);
    }

    #[test]
    fn test_denormalize_cache_path_no_slash_in_first_component() {
        // If the first component has no `/`, it should remain unchanged.
        let input = Path::new("singlecomponent/@v/v1.0.0.info");
        let expected = PathBuf::from("singlecomponent/@v/v1.0.0.info");
        assert_eq!(denormalize_cache_path(input), expected);
    }

    // ----------------------- Test: is_idempotent_skip -----------------------

    #[test]
    fn test_is_idempotent_skip_existing_same_size() {
        let tmp = TempDir::new().expect("tempdir failed");
        let file_path = tmp.path().join("test.txt");

        let content = b"hello";
        fs::write(&file_path, content).expect("write failed");

        // Size is 5 bytes, same as expected.
        assert!(is_idempotent_skip(&file_path, 5));
    }

    #[test]
    fn test_is_idempotent_skip_existing_different_size() {
        let tmp = TempDir::new().expect("tempdir failed");
        let file_path = tmp.path().join("test.txt");

        fs::write(&file_path, b"hello").expect("write failed");

        // Size is 5 but expected is 10.
        assert!(!is_idempotent_skip(&file_path, 10));
    }

    #[test]
    fn test_is_idempotent_skip_nonexistent() {
        let tmp = TempDir::new().expect("tempdir failed");
        let file_path = tmp.path().join("nonexistent.txt");

        assert!(!is_idempotent_skip(&file_path, 5));
    }

    // ----------------------- Test: entries with leading ./ prefix -----------------------

    #[test]
    fn test_entries_with_leading_dot_slash() {
        let tmp = TempDir::new().expect("tempdir failed");
        let archive_path = tmp.path().join("dot.zip");
        let target = tmp.path().join("cache");

        let zip_data = build_zip(&[(
            "./github.com/test/mod/@v/v1.0.0.info",
            b"content",
        )]);
        write_zip_to_file(&archive_path, &zip_data);

        let stats = unpack_modules(&archive_path, &target).expect("unpack failed");

        assert_eq!(stats.extracted, 1);
        assert_eq!(stats.failed, 0);

        let extracted = target.join("github.com!test!mod/@v/v1.0.0.info");
        assert!(extracted.exists());
    }

    // ----------------------- Test: mixed success and failure -----------------------

    #[test]
    fn test_mixed_success_and_traversal() {
        let tmp = TempDir::new().expect("tempdir failed");
        let archive_path = tmp.path().join("mixed.zip");
        let target = tmp.path().join("cache");

        let zip_data = build_zip(&[
            ("github.com/good/mod/@v/v1.0.0.info", b"good"),
            ("../escape/attempt", b"evil"),
            ("github.com/also/good/@v/v2.0.0.info", b"also good"),
        ]);
        write_zip_to_file(&archive_path, &zip_data);

        let stats = unpack_modules(&archive_path, &target).expect("unpack failed");

        assert_eq!(stats.extracted, 2);
        assert_eq!(stats.failed, 1);
        assert_eq!(stats.skipped, 0);
    }
}
