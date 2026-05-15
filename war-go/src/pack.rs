//! pack - Archive the Go module cache for airgap transfer.
//!
//! Walks the Go module cache directory (typically `~/.war/cache/go/`),
//! normalizes `!`-separated directory names back to `/`-separated paths
//! for the zip archive, and writes a portable `.zip` file.  Supports
//! an optional `staged_filter` so `war go pack --staged` only includes
//! modules present in `~/.war/war.lock`.
//!
//! ## Phase 6 additions
//! - `tracing::info_span!` wraps the entire operation so `--verbose` shows
//!   timing in structured logs.
//! - File-count progress is logged every 50 files and at completion.
//! - Zip errors are mapped to `WarError::ZipCreationError` with the full
//!   module path, so the CLI can surface actionable hints.
//! - Output file is written to a `.war.tmp` sidecar and renamed atomically,
//!   preventing corrupt archives from being left at the output path on failure.

use std::{
    fs::File,
    io::{self, BufWriter},
    path::Path,
    time::Instant,
};
use walkdir::WalkDir;
use war_core::error::WarError;
use zip::{write::FileOptions, ZipWriter};

// -------------------------------------------- Public API --------------------------------------------

/// Pack Go module cache into a zip archive.
///
/// When `staged_filter` is `Some`, only files belonging to modules in
/// the filter list are included in the archive.  The filter consists of
/// `(module_path, version)` pairs as stored in `GoConfig::staged_modules`.
///
/// The archive is written atomically: content streams into `output.war.tmp`
/// and is `rename`d into `output` only on full success.  A failed pack
/// therefore never leaves a truncated zip at the destination.
///
/// # Errors
///
/// - `WarError::InvalidInput` — `cache_root` does not exist.
/// - `WarError::IOError` — filesystem errors during the walk or tmp-file creation.
/// - `WarError::ZipCreationError` — zip encoding failure for a specific file.
pub async fn pack_modules(
    cache_root: &Path,
    output: &Path,
    staged_filter: Option<Vec<(String, String)>>,
) -> Result<(), WarError> {
    let span = tracing::info_span!(
        "pack_modules",
        cache = %cache_root.display(),
        output = %output.display(),
        staged = staged_filter.is_some(),
    );
    let _enter = span.enter();

    if !cache_root.exists() {
        return Err(WarError::InvalidInput(format!(
            "Cache root does not exist: {}. \
             Run `war go unpack <archive>` first.",
            cache_root.display()
        )));
    }

    let t0 = Instant::now();

    // Atomic write: stream into a .war.tmp sidecar, rename on success.
    let tmp_output = output.with_extension({
        let ext = output.extension().and_then(|e| e.to_str()).unwrap_or("zip");
        format!("{}.war.tmp", ext)
    });

    // Ensure the output parent directory exists.
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let file = File::create(&tmp_output).map_err(|e| {
        WarError::IOError(io::Error::new(
            e.kind(),
            format!(
                "Cannot create temporary output file at {}: {}",
                tmp_output.display(),
                e
            ),
        ))
    })?;

    let writer = BufWriter::new(file);
    let mut zip = ZipWriter::new(writer);

    let options: FileOptions<'_, ()> = FileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .unix_permissions(0o755);

    let mut file_count: usize = 0;
    let mut skipped_count: usize = 0;
    const PROGRESS_INTERVAL: usize = 50;

    tracing::info!("(◕‿◕✿) Walking cache at {} …", cache_root.display());

    for entry in WalkDir::new(cache_root) {
        let entry = entry.map_err(|e| {
            WarError::IOError(io::Error::new(
                io::ErrorKind::Other,
                format!("Failed to walk cache directory: {}", e),
            ))
        })?;

        let path = entry.path();
        if !path.is_file() {
            continue;
        }

        // Apply staged filter if present.
        if let Some(filter) = &staged_filter {
            if !matches_filter(cache_root, path, filter)? {
                skipped_count += 1;
                continue;
            }
        }

        let relative = path.strip_prefix(cache_root).map_err(|e| {
            WarError::InvalidInput(format!(
                "Path {} is not under cache root {}: {}",
                path.display(),
                cache_root.display(),
                e
            ))
        })?;

        let zip_path = normalize_cache_path(relative);

        tracing::debug!("  + {}", zip_path);

        zip.start_file(&zip_path, options)
            .map_err(|e| WarError::ZipCreationError {
                module: zip_path.clone(),
                source: e,
            })?;

        let mut f = File::open(path)?;
        io::copy(&mut f, &mut zip)?;

        file_count += 1;

        // Progress heartbeat every N files.
        if file_count % PROGRESS_INTERVAL == 0 {
            tracing::info!("  … {} files added to archive so far …", file_count);
        }
    }

    // Finish the zip before renaming so the central directory is flushed.
    zip.finish().map_err(|e| WarError::ZipCreationError {
        module: cache_root.display().to_string(),
        source: e,
    })?;

    // Atomic rename: only now does the output file become visible.
    std::fs::rename(&tmp_output, output).map_err(|e| {
        // Best-effort cleanup of the tmp on rename failure.
        let _ = std::fs::remove_file(&tmp_output);
        WarError::IOError(io::Error::new(
            e.kind(),
            format!(
                "Atomic rename of {} → {} failed: {}. \
                 A partial .war.tmp file may remain — safe to delete.",
                tmp_output.display(),
                output.display(),
                e
            ),
        ))
    })?;

    let elapsed = t0.elapsed();

    if let Some(filter) = &staged_filter {
        tracing::info!(
            "✔ Archive written in {:.2}s — {} file(s) packed, {} skipped (--staged: {} module(s)) → {}",
            elapsed.as_secs_f64(),
            file_count,
            skipped_count,
            filter.len(),
            output.display()
        );
    } else {
        tracing::info!(
            "✔ Archive written in {:.2}s — {} file(s) packed → {}",
            elapsed.as_secs_f64(),
            file_count,
            output.display()
        );
    }

    Ok(())
}

// -------------------------------------------- Internal Helpers --------------------------------------------

/// Check whether a file in the cache belongs to a module in the staged
/// filter list.
///
/// The filter is a list of `(module_path, version)` pairs.  The function
/// extracts the module and version from the file's path relative to the
/// cache root and checks for membership.
///
/// # Path structure
///
/// ```text
/// github.com!gin-gonic!gin/@v/v1.9.1.info
/// │── module parts (joined with !) ──│ │v│ │version+ext│
/// ```
///
/// The `@v` directory is the boundary marker.  Everything before `@v`
/// is the module path (with `!` → `/` conversion).  The filename under
/// `@v` provides the version (before the extension).
///
/// # Special case: "latest" version
///
/// If the staged filter specifies version `"latest"`, it matches ANY
/// version of that module in the cache.  This handles the common workflow
/// where users run `war go get module@latest` and expect all fetched
/// versions to be included in `pack --staged`.
fn matches_filter(
    cache_root: &Path,
    file: &Path,
    filter: &[(String, String)],
) -> Result<bool, WarError> {
    let relative = file.strip_prefix(cache_root).map_err(|e| {
        WarError::InvalidInput(format!(
            "Path {} not under cache root {}: {}",
            file.display(),
            cache_root.display(),
            e
        ))
    })?;

    let parts: Vec<_> = relative.components().collect();
    if parts.len() < 3 {
        return Ok(false);
    }

    let v_idx = parts
        .iter()
        .position(|c| c.as_os_str() == "@v")
        .ok_or_else(|| WarError::InvalidInput("Invalid cache path (missing @v)".into()))?;

    if v_idx + 1 >= parts.len() {
        return Ok(false);
    }

    let module = parts[..v_idx]
        .iter()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
        .replace("!", "/");

    let version = Path::new(parts[v_idx + 1].as_os_str())
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_string();

    // Check for exact match OR "latest" wildcard match
    Ok(filter.iter().any(|(f_module, f_version)| {
        f_module == &module && (f_version == "latest" || f_version == &version)
    }))
}

/// Convert a Go cache relative path (with `!` separators in the first
/// component) into an archive-friendly path (with `/` separators).
///
/// Example: `github.com!gin-gonic!gin/@v/v1.9.1.info`
///       → `github.com/gin-gonic/gin/@v/v1.9.1.info`
fn normalize_cache_path(relative: &Path) -> String {
    let mut parts = relative
        .components()
        .map(|c| c.as_os_str().to_string_lossy());
    let first = parts.next().unwrap_or_default().replace("!", "/");
    let mut out = String::from(first);
    for p in parts {
        out.push('/');
        out.push_str(&p);
    }
    out
}

// -------------------------------------------- Tests --------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn test_normalize_cache_path_basic() {
        let input = Path::new("github.com!gin-gonic!gin/@v/v1.9.1.info");
        assert_eq!(
            normalize_cache_path(input),
            "github.com/gin-gonic/gin/@v/v1.9.1.info"
        );
    }

    #[test]
    fn test_normalize_cache_path_single_component() {
        let input = Path::new("singlemod/@v/v1.0.0.mod");
        assert_eq!(normalize_cache_path(input), "singlemod/@v/v1.0.0.mod");
    }

    #[test]
    fn test_matches_filter_positive() {
        let dir = tempdir().unwrap();
        let cache = dir.path().join("cache");
        fs::create_dir_all(cache.join("github.com!gin-gonic!gin/@v")).unwrap();

        let file_path = cache.join("github.com!gin-gonic!gin/@v/v1.9.1.info");
        fs::write(&file_path, b"{}").unwrap();

        let filter = vec![("github.com/gin-gonic/gin".to_string(), "v1.9.1".to_string())];
        assert!(matches_filter(&cache, &file_path, &filter).unwrap());
    }

    #[test]
    fn test_matches_filter_negative() {
        let dir = tempdir().unwrap();
        let cache = dir.path().join("cache");
        fs::create_dir_all(cache.join("github.com!other!mod/@v")).unwrap();
        let file_path = cache.join("github.com!other!mod/@v/v2.0.0.info");
        fs::write(&file_path, b"{}").unwrap();

        let filter = vec![("github.com/gin-gonic/gin".to_string(), "v1.9.1".to_string())];
        assert!(!matches_filter(&cache, &file_path, &filter).unwrap());
    }

    /// Verify that a failed pack does NOT leave a partial zip at the output path.
    #[tokio::test]
    async fn test_atomic_pack_does_not_leave_partial_on_missing_cache() {
        let dir = tempdir().unwrap();
        let missing_cache = dir.path().join("no_such_cache");
        let output = dir.path().join("out.zip");

        let result = pack_modules(&missing_cache, &output, None).await;

        assert!(result.is_err(), "expected error for missing cache");
        assert!(!output.exists(), "output must not exist after failed pack");

        // Tmp file must also be cleaned up.
        let tmp = dir.path().join("out.zip.war.tmp");
        assert!(!tmp.exists(), "tmp file must not remain after early error");
    }
}
