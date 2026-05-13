//! sync - Permanently hydrate the native Go module cache (`$GOMODCACHE`) from
//! the war cache at `~/.war/cache/go`, enabling `go build` without `eval $(war go offline)`.
//!
//! # Overview
//!
//! After `war go unpack cache.zip`, modules live in `~/.war/cache/go` — war's
//! private GOPROXY-style cache.  `war go sync` copies them into Go's **native**
//! module cache (`~/go/pkg/mod/cache/download` by default, or whatever
//! `$GOMODCACHE` is set to).  Once synced, standard tooling (`go build`, `go test`,
//! `go mod tidy`) resolves modules from the native cache without any extra env var
//! gymnastics.
//!
//! # Safety Guarantees
//!
//! - **Idempotent**: files already present with matching size + SHA-256 are skipped.
//! - **Atomic writes**: every file is written to a `.war.tmp` sidecar in the same
//!   directory and then `fs::rename`d into place.  Interrupted syncs leave no
//!   corrupt partial files.
//! - **Collision detection**: if a destination file exists but has a *different*
//!   SHA-256 hash from the source, war warns and skips rather than silently
//!   overwriting a file the user may have modified.
//! - **Never overwrites modified files**: the caller gets a `CollisionWarning` for
//!   every hash mismatch so it can surface them in the CLI output.

use std::{
    fs,
    io::{self, Read, Write},
    path::{Component, Path, PathBuf},
};
use walkdir::WalkDir;
use war_core::WarError;

// -------------------------------------------- Types --------------------------------------------

/// Aggregate statistics returned after a `sync_cache` run.
///
/// Mirrors the shape of `UnpackStats` so callers can use the same display
/// logic for both operations.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SyncStats {
    /// Files successfully copied to the native Go module cache.
    pub copied: usize,
    /// Files skipped because the destination already matched (size + SHA-256).
    pub skipped: usize,
    /// Files that could not be copied due to I/O errors.
    pub failed: usize,
    /// Files where a hash collision was detected (destination exists but differs).
    ///
    /// These are *not* counted in `failed`; the operation is intentionally
    /// non-destructive — the user must resolve collisions manually.
    pub collisions: usize,
}

/// A single collision warning emitted when the destination file differs from
/// the source file.
///
/// Collisions do **not** abort the sync.  They are collected and returned
/// alongside `SyncStats` so the CLI can surface them as warnings.
#[derive(Debug, Clone)]
pub struct CollisionWarning {
    /// Path of the source file in `~/.war/cache/go`.
    pub source: PathBuf,
    /// Path of the destination file in the native Go module cache.
    pub destination: PathBuf,
    /// SHA-256 hex digest of the **source** file.
    pub source_hash: String,
    /// SHA-256 hex digest of the **destination** file.
    pub destination_hash: String,
}

/// The full result of a sync operation: statistics plus any collision warnings.
#[derive(Debug, Default)]
pub struct SyncResult {
    /// Aggregate copy/skip/fail/collision counts.
    pub stats: SyncStats,
    /// Per-file collision details (empty when there are no conflicts).
    pub warnings: Vec<CollisionWarning>,
}

// -------------------------------------------- Public Functions --------------------------------------------

/// Synchronise `~/.war/cache/go` → native Go module cache (`$GOMODCACHE`).
///
/// Walks every file under `src_root` (the war cache), computes the
/// destination path under `dst_root` (the native GOMODCACHE), and copies
/// the file with atomic-rename semantics.
///
/// # Arguments
///
/// * `src_root` — war's Go cache root, typically `~/.war/cache/go`.
/// * `dst_root` — the native Go module cache root, typically `~/go/pkg/mod/cache/download`.
///
/// # Returns
///
/// A `SyncResult` with aggregate stats and any collision warnings.
/// The operation continues past individual file errors; callers inspect
/// `stats.failed` and `stats.collisions` to decide whether to surface a
/// non-zero exit code.
///
/// # Errors
///
/// Returns `WarError::InvalidInput` if `src_root` does not exist.
/// Returns `WarError::IOError` if the destination root cannot be created.
pub fn sync_cache(src_root: &Path, dst_root: &Path) -> Result<SyncResult, WarError> {
    if !src_root.exists() {
        return Err(WarError::InvalidInput(format!(
            "War cache source does not exist: {}\n\
             Run `war go unpack <archive>` first (◕‿◕✿)",
            src_root.display()
        )));
    }

    tracing::info!(
        "(◕‿◕✿) Starting sync: {} → {}",
        src_root.display(),
        dst_root.display()
    );

    // Ensure the destination root exists before we start walking.
    fs::create_dir_all(dst_root)?;

    let mut result = SyncResult::default();

    for entry in WalkDir::new(src_root)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| match e {
            Ok(entry) => Some(entry),
            Err(err) => {
                tracing::warn!("⚠ Failed to walk entry: {}", err);
                None
            }
        })
    {
        // Skip directory entries — we create them on demand during file copy.
        if entry.file_type().is_dir() {
            continue;
        }

        let src_path = entry.path();

        // Compute the relative path from src_root and re-root it under dst_root.
        let relative = match src_path.strip_prefix(src_root) {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(
                    "⚠ Unexpected strip_prefix failure for {}: {}",
                    src_path.display(),
                    e
                );
                result.stats.failed += 1;
                continue;
            }
        };

        // Security: reject any path that escaped the source root via `..`.
        if contains_traversal(relative) {
            tracing::warn!("⚠ Rejecting traversal path: {}", relative.display());
            result.stats.failed += 1;
            continue;
        }

        let dst_path = dst_root.join(relative);

        match sync_single_file(src_path, &dst_path) {
            Ok(FileOutcome::Copied) => {
                tracing::info!("✔ Copied: {}", relative.display());
                result.stats.copied += 1;
            }
            Ok(FileOutcome::Skipped) => {
                tracing::debug!("– Skipped (already up-to-date): {}", relative.display());
                result.stats.skipped += 1;
            }
            Ok(FileOutcome::Collision(warning)) => {
                tracing::warn!(
                    "⚠ Collision detected at {} — source hash {} ≠ destination hash {}. \
                     Skipping to protect existing file.",
                    relative.display(),
                    warning.source_hash,
                    warning.destination_hash
                );
                result.stats.collisions += 1;
                result.warnings.push(warning);
            }
            Err(e) => {
                tracing::warn!("⚠ Failed to copy {}: {}", relative.display(), e);
                result.stats.failed += 1;
            }
        }
    }

    tracing::info!(
        "(≧◡≦) Sync complete: {} copied, {} skipped, {} collisions, {} failed",
        result.stats.copied,
        result.stats.skipped,
        result.stats.collisions,
        result.stats.failed
    );

    Ok(result)
}

/// Resolve the native Go module cache root, respecting `$GOMODCACHE` if set.
///
/// Resolution order:
/// 1. `$GOMODCACHE` — explicit override set by the user or CI.
/// 2. `$GOPATH/pkg/mod` — classic layout.
/// 3. `~/go/pkg/mod` — the default GOPATH introduced in Go 1.13.
///
/// The returned path is the `cache/download` sub-directory, which is the
/// GOPROXY-protocol tree that `war go unpack` populates.
pub fn resolve_gomodcache() -> Result<PathBuf, WarError> {
    // 1. $GOMODCACHE takes highest priority.
    if let Ok(gomodcache) = std::env::var("GOMODCACHE") {
        if !gomodcache.is_empty() {
            let path = PathBuf::from(&gomodcache);
            tracing::debug!("Using GOMODCACHE from env: {}", path.display());
            return Ok(path.join("cache").join("download"));
        }
    }

    // 2. $GOPATH/pkg/mod (classic, explicit GOPATH).
    if let Ok(gopath) = std::env::var("GOPATH") {
        if !gopath.is_empty() {
            let path = PathBuf::from(&gopath).join("pkg").join("mod");
            tracing::debug!("Using GOPATH from env: {}", path.display());
            return Ok(path.join("cache").join("download"));
        }
    }

    // 3. ~/go/pkg/mod (Go 1.13+ default).
    let home = dirs::home_dir().ok_or_else(|| {
        WarError::InvalidInput("Cannot determine home directory to resolve GOMODCACHE".to_string())
    })?;

    let default_path = home.join("go").join("pkg").join("mod");
    tracing::debug!("Using default GOPATH: {}", default_path.display());
    Ok(default_path.join("cache").join("download"))
}

// -------------------------------------------- Internal Helpers --------------------------------------------

/// The outcome of attempting to sync a single file.
enum FileOutcome {
    /// The file was successfully copied to the destination.
    Copied,
    /// The destination already existed with matching content — no action taken.
    Skipped,
    /// The destination exists but has a different hash — collision, not copied.
    Collision(CollisionWarning),
}

/// Sync a single file from `src` to `dst` with full safety guarantees.
///
/// Decision tree:
/// 1. `dst` does not exist → atomic copy.
/// 2. `dst` exists, same size → SHA-256 compare.
///    a. Hashes match → `Skipped` (idempotent).
///    b. Hashes differ → `Collision` (warn, do not overwrite).
/// 3. `dst` exists, different size → treat as content mismatch → `Collision`.
fn sync_single_file(src: &Path, dst: &Path) -> Result<FileOutcome, WarError> {
    // Ensure the destination parent directory exists.
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent)?;
    }

    // Fast path: destination does not exist → just copy.
    if !dst.exists() {
        atomic_copy(src, dst)?;
        return Ok(FileOutcome::Copied);
    }

    // Destination exists: compare sizes first as a cheap pre-filter.
    let src_meta = fs::metadata(src)?;
    let dst_meta = fs::metadata(dst)?;

    if src_meta.len() != dst_meta.len() {
        // Sizes differ → definitely not the same file → collision.
        let src_hash = sha256_file(src)?;
        let dst_hash = sha256_file(dst)?;
        return Ok(FileOutcome::Collision(CollisionWarning {
            source: src.to_path_buf(),
            destination: dst.to_path_buf(),
            source_hash: src_hash,
            destination_hash: dst_hash,
        }));
    }

    // Same size → deep compare with SHA-256.
    let src_hash = sha256_file(src)?;
    let dst_hash = sha256_file(dst)?;

    if src_hash == dst_hash {
        return Ok(FileOutcome::Skipped);
    }

    // Same size but different hash → treated as a collision.
    Ok(FileOutcome::Collision(CollisionWarning {
        source: src.to_path_buf(),
        destination: dst.to_path_buf(),
        source_hash: src_hash,
        destination_hash: dst_hash,
    }))
}

/// Copy `src` → `dst` atomically using a `.war.tmp` sidecar file.
///
/// The file is written to `dst.war.tmp` in the same directory as `dst`,
/// then `fs::rename`d into place.  On POSIX systems `rename(2)` is atomic
/// with respect to the directory entry, so concurrent readers never see a
/// partial file.
///
/// If a stale `.war.tmp` leftover exists from a previous crashed run it is
/// silently overwritten — the sidecar is never the final artifact.
fn atomic_copy(src: &Path, dst: &Path) -> Result<(), WarError> {
    // Build the temporary path next to the destination so the rename stays
    // on the same filesystem mount point (required for atomic rename).
    let tmp_path = {
        let mut p = dst.to_path_buf();
        let ext = p
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("dat")
            .to_owned();
        p.set_extension(format!("{}.war.tmp", ext));
        p
    };

    // Open source, create tmp, stream bytes.
    let mut src_file = fs::File::open(src)?;
    let mut tmp_file = fs::File::create(&tmp_path)?;

    io::copy(&mut src_file, &mut tmp_file)?;
    tmp_file.flush()?;
    drop(tmp_file); // close before rename on Windows

    // Atomic rename into final position.
    fs::rename(&tmp_path, dst).map_err(|e| {
        // Best-effort cleanup of the tmp file on rename failure.
        let _ = fs::remove_file(&tmp_path);
        WarError::IOError(io::Error::new(
            e.kind(),
            format!(
                "Atomic rename failed: {} → {}: {}",
                tmp_path.display(),
                dst.display(),
                e
            ),
        ))
    })?;

    Ok(())
}

/// Compute the SHA-256 hex digest of a file, streaming to avoid loading the
/// entire file into memory (important for large `.zip` module archives).
///
/// Returns a lowercase hex string of 64 characters.
fn sha256_file(path: &Path) -> Result<String, WarError> {
    let mut file = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024]; // 64 KiB read buffer

    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }

    Ok(hasher.finalize_hex())
}

/// Check whether a relative path contains `..` (parent directory traversal).
///
/// Security-critical: called on every path derived from walkdir before it is
/// joined with the destination root.
fn contains_traversal(path: &Path) -> bool {
    path.components().any(|c| matches!(c, Component::ParentDir))
}

// -------------------------------------------- Minimal SHA-256 (no extra dep) --------------------------------------------

/// A minimal, dependency-free SHA-256 implementation used for file comparison.
///
/// We deliberately avoid pulling in `sha2` / `ring` / `digest` crates so
/// that `war-go`'s dependency footprint stays small.  This is the standard
/// FIPS 180-4 SHA-256 algorithm — correct and sufficient for file-identity
/// checks.
struct Sha256 {
    state: [u32; 8],
    buf: [u8; 64],
    buf_len: usize,
    total_bits: u64,
}

impl Sha256 {
    /// SHA-256 initial hash values (first 32 bits of fractional parts of √primes).
    const H: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];

    /// SHA-256 round constants (first 32 bits of fractional parts of ∛primes).
    #[rustfmt::skip]
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5,
        0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
        0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3,
        0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
        0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc,
        0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
        0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
        0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13,
        0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
        0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3,
        0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
        0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5,
        0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208,
        0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
    ];

    /// Construct a new SHA-256 hasher with the standard initial state.
    fn new() -> Self {
        Self {
            state: Self::H,
            buf: [0u8; 64],
            buf_len: 0,
            total_bits: 0,
        }
    }

    /// Feed bytes into the hasher.
    fn update(&mut self, data: &[u8]) {
        self.total_bits += (data.len() as u64) * 8;

        let mut offset = 0;

        // Fill and process the internal buffer.
        while offset < data.len() {
            let space = 64 - self.buf_len;
            let take = space.min(data.len() - offset);
            self.buf[self.buf_len..self.buf_len + take]
                .copy_from_slice(&data[offset..offset + take]);
            self.buf_len += take;
            offset += take;

            if self.buf_len == 64 {
                let block = self.buf;
                self.compress(&block);
                self.buf_len = 0;
            }
        }
    }

    /// Finalise the digest and return a 64-character lowercase hex string.
    fn finalize_hex(mut self) -> String {
        // Padding: append 0x80 byte.
        let total_bits = self.total_bits;
        self.update(&[0x80]);

        // Pad to 56 bytes mod 64 so there is room for the 8-byte length.
        while self.buf_len % 64 != 56 {
            self.update(&[0x00]);
        }

        // Append message length as big-endian u64.
        let len_bytes = total_bits.to_be_bytes();
        self.update(&len_bytes);

        // Encode state as big-endian hex.
        self.state
            .iter()
            .flat_map(|word| {
                let b = word.to_be_bytes();
                [
                    hex_nibble(b[0] >> 4),
                    hex_nibble(b[0] & 0xf),
                    hex_nibble(b[1] >> 4),
                    hex_nibble(b[1] & 0xf),
                    hex_nibble(b[2] >> 4),
                    hex_nibble(b[2] & 0xf),
                    hex_nibble(b[3] >> 4),
                    hex_nibble(b[3] & 0xf),
                ]
            })
            .map(|c| c as char)
            .collect()
    }

    /// Process a single 64-byte block using the SHA-256 compression function.
    fn compress(&mut self, block: &[u8; 64]) {
        // Build the 64-word message schedule.
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([
                block[i * 4],
                block[i * 4 + 1],
                block[i * 4 + 2],
                block[i * 4 + 3],
            ]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }

        // Initialise working variables from current state.
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = self.state;

        // 64 rounds.
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let temp1 = h
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(Self::K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(maj);

            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }

        // Add compressed chunk to current state.
        self.state[0] = self.state[0].wrapping_add(a);
        self.state[1] = self.state[1].wrapping_add(b);
        self.state[2] = self.state[2].wrapping_add(c);
        self.state[3] = self.state[3].wrapping_add(d);
        self.state[4] = self.state[4].wrapping_add(e);
        self.state[5] = self.state[5].wrapping_add(f);
        self.state[6] = self.state[6].wrapping_add(g);
        self.state[7] = self.state[7].wrapping_add(h);
    }
}

/// Convert a nibble (0–15) to its lowercase ASCII hex character.
#[inline]
fn hex_nibble(n: u8) -> u8 {
    match n {
        0..=9 => b'0' + n,
        _ => b'a' + (n - 10),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    // ---- Helper: build a minimal war cache tree ----

    /// Create a fake war cache directory with the given `(relative_path, content)` files.
    fn build_cache(dir: &Path, files: &[(&str, &[u8])]) {
        for (rel, content) in files {
            let dest = dir.join(rel);
            fs::create_dir_all(dest.parent().unwrap()).unwrap();
            fs::write(&dest, content).unwrap();
        }
    }

    // ---- SHA-256 unit tests ----

    #[test]
    fn test_sha256_empty() {
        // SHA-256("") = e3b0c44298fc1c149afb...
        let mut h = Sha256::new();
        h.update(b"");
        let hex = h.finalize_hex();
        assert_eq!(&hex[..8], "e3b0c442");
    }

    #[test]
    fn test_sha256_abc() {
        // SHA-256("abc") = ba7816bf8f01cfea414140de5dae2ec73b00361bbef0469...
        let mut h = Sha256::new();
        h.update(b"abc");
        let hex = h.finalize_hex();
        assert_eq!(&hex[..8], "ba7816bf");
        assert_eq!(hex.len(), 64);
    }

    #[test]
    fn test_sha256_longer_message() {
        // SHA-256("The quick brown fox jumps over the lazy dog")
        // = d7a8fbb307d7809469ca9abcb0082e4f8d5651e46d3cdb762d02d0bf37c9e592
        let mut h = Sha256::new();
        h.update(b"The quick brown fox jumps over the lazy dog");
        let hex = h.finalize_hex();
        assert_eq!(&hex[..8], "d7a8fbb3");
    }

    #[test]
    fn test_sha256_multi_update() {
        // Feeding data in chunks must produce the same digest as a single update.
        let data = b"hello world";

        let mut h1 = Sha256::new();
        h1.update(data);
        let full = h1.finalize_hex();

        let mut h2 = Sha256::new();
        h2.update(b"hello");
        h2.update(b" ");
        h2.update(b"world");
        let chunked = h2.finalize_hex();

        assert_eq!(full, chunked);
    }

    // ---- Copy logic ----

    #[test]
    fn test_sync_copies_new_files() {
        let tmp = TempDir::new().unwrap();
        let src = tmp.path().join("src");
        let dst = tmp.path().join("dst");

        build_cache(
            &src,
            &[
                (
                    "github.com!gin-gonic!gin/@v/v1.9.1.info",
                    b"{\"Version\":\"v1.9.1\"}",
                ),
                (
                    "github.com!gin-gonic!gin/@v/v1.9.1.mod",
                    b"module github.com/gin-gonic/gin\n",
                ),
            ],
        );

        let result = sync_cache(&src, &dst).expect("sync failed");

        assert_eq!(result.stats.copied, 2);
        assert_eq!(result.stats.skipped, 0);
        assert_eq!(result.stats.failed, 0);
        assert_eq!(result.stats.collisions, 0);

        assert!(dst.join("github.com!gin-gonic!gin/@v/v1.9.1.info").exists());
        assert!(dst.join("github.com!gin-gonic!gin/@v/v1.9.1.mod").exists());
    }

    // ---- Skip-on-match (idempotency) ----

    #[test]
    fn test_sync_skips_identical_files() {
        let tmp = TempDir::new().unwrap();
        let src = tmp.path().join("src");
        let dst = tmp.path().join("dst");

        let content = b"module github.com/gin-gonic/gin\n";
        build_cache(&src, &[("gin/@v/v1.9.1.mod", content)]);
        build_cache(&dst, &[("gin/@v/v1.9.1.mod", content)]);

        let result = sync_cache(&src, &dst).expect("sync failed");

        assert_eq!(result.stats.copied, 0, "expected skip, not copy");
        assert_eq!(result.stats.skipped, 1);
        assert_eq!(result.stats.collisions, 0);
    }

    // ---- Collision detection ----

    #[test]
    fn test_sync_warns_on_hash_collision() {
        let tmp = TempDir::new().unwrap();
        let src = tmp.path().join("src");
        let dst = tmp.path().join("dst");

        // Same file name, same size, different content → hash mismatch.
        let src_content = b"version: war-synced-content!";
        let dst_content = b"version: user-modified-stuff";
        assert_eq!(
            src_content.len(),
            dst_content.len(),
            "test requires same-size different-content files"
        );

        build_cache(&src, &[("mod/@v/v1.0.0.info", src_content)]);
        build_cache(&dst, &[("mod/@v/v1.0.0.info", dst_content)]);

        let result = sync_cache(&src, &dst).expect("sync failed");

        assert_eq!(result.stats.collisions, 1, "expected exactly one collision");
        assert_eq!(result.stats.copied, 0, "should NOT overwrite on collision");
        assert_eq!(result.warnings.len(), 1);

        // Destination file must remain untouched.
        let dst_file = dst.join("mod/@v/v1.0.0.info");
        let on_disk = fs::read(&dst_file).unwrap();
        assert_eq!(on_disk, dst_content, "collision must not overwrite file");
    }

    // ---- Collision: different size ----

    #[test]
    fn test_sync_warns_on_size_mismatch_collision() {
        let tmp = TempDir::new().unwrap();
        let src = tmp.path().join("src");
        let dst = tmp.path().join("dst");

        build_cache(&src, &[("m/@v/v1.0.0.mod", b"module x\n\ngo 1.22\n")]);
        build_cache(&dst, &[("m/@v/v1.0.0.mod", b"module x\n")]);

        let result = sync_cache(&src, &dst).expect("sync failed");

        assert_eq!(result.stats.collisions, 1);
        assert_eq!(result.stats.copied, 0);
    }

    // ---- Atomic rename: verify .war.tmp is cleaned up ----

    #[test]
    fn test_atomic_copy_leaves_no_tmp_file() {
        let tmp = TempDir::new().unwrap();
        let src = tmp.path().join("src.mod");
        let dst = tmp.path().join("dst.mod");

        fs::write(&src, b"module test\n").unwrap();
        atomic_copy(&src, &dst).expect("atomic_copy failed");

        assert!(dst.exists(), "destination must exist after copy");

        // The .war.tmp sidecar must be gone after a successful rename.
        let tmp_file = tmp.path().join("dst.mod.war.tmp");
        assert!(
            !tmp_file.exists(),
            "temp file should be cleaned up after rename"
        );

        let content = fs::read_to_string(&dst).unwrap();
        assert_eq!(content, "module test\n");
    }

    // ---- Missing source ----

    #[test]
    fn test_sync_error_on_missing_src() {
        let tmp = TempDir::new().unwrap();
        let src = tmp.path().join("nonexistent");
        let dst = tmp.path().join("dst");

        let result = sync_cache(&src, &dst);

        assert!(result.is_err());
        match result.unwrap_err() {
            WarError::InvalidInput(msg) => {
                assert!(
                    msg.contains("War cache source does not exist"),
                    "unexpected msg: {}",
                    msg
                );
            }
            other => panic!("expected InvalidInput, got: {:?}", other),
        }
    }

    // ---- resolve_gomodcache ----

    #[test]
    fn test_resolve_gomodcache_from_env() {
        // We temporarily override GOMODCACHE to test resolution.
        // Use a distinct key to avoid flaking with the real environment.
        let prev = std::env::var("GOMODCACHE").ok();
        std::env::set_var("GOMODCACHE", "/custom/gomod");

        let result = resolve_gomodcache().expect("resolve failed");

        assert_eq!(
            result,
            PathBuf::from("/custom/gomod/cache/download"),
            "GOMODCACHE env must take priority"
        );

        // Restore.
        match prev {
            Some(v) => std::env::set_var("GOMODCACHE", v),
            None => std::env::remove_var("GOMODCACHE"),
        }
    }

    // ---- contains_traversal ----

    #[test]
    fn test_contains_traversal_rejects_parent() {
        assert!(contains_traversal(Path::new("../../etc/passwd")));
        assert!(contains_traversal(Path::new("foo/../bar")));
        assert!(!contains_traversal(Path::new(
            "github.com!gin-gonic!gin/@v/v1.9.1.info"
        )));
    }

    // ---- Integration: full cache → native layout ----

    #[test]
    fn test_integration_sync_cache_to_native_layout() {
        let tmp = TempDir::new().unwrap();
        let war_cache = tmp.path().join("war_cache");
        let native_cache = tmp.path().join("go_pkg_mod_cache_download");

        // Simulate what `war go unpack` produces in ~/.war/cache/go:
        // Go's GOPROXY file tree uses !-encoded module paths.
        build_cache(
            &war_cache,
            &[
                (
                    "github.com!gin-gonic!gin/@v/v1.9.1.info",
                    b"{\"Version\":\"v1.9.1\",\"Time\":\"2025-01-01T00:00:00Z\"}",
                ),
                (
                    "github.com!gin-gonic!gin/@v/v1.9.1.mod",
                    b"module github.com/gin-gonic/gin\n\ngo 1.20\n",
                ),
                (
                    "github.com!gin-gonic!gin/@v/v1.9.1.zip",
                    b"PK\x03\x04fake-zip-content",
                ),
                (
                    "golang.org!x!text/@v/v0.3.7.info",
                    b"{\"Version\":\"v0.3.7\",\"Time\":\"2025-01-01T00:00:00Z\"}",
                ),
                (
                    "golang.org!x!text/@v/v0.3.7.mod",
                    b"module golang.org/x/text\n\ngo 1.17\n",
                ),
            ],
        );

        let result = sync_cache(&war_cache, &native_cache).expect("sync failed");

        // All 5 files must be copied, none skipped or collided.
        assert_eq!(result.stats.copied, 5, "all files must be copied");
        assert_eq!(result.stats.skipped, 0);
        assert_eq!(result.stats.collisions, 0);
        assert_eq!(result.stats.failed, 0);

        // Verify the native cache has the correct GOPROXY-protocol layout.
        assert!(native_cache
            .join("github.com!gin-gonic!gin/@v/v1.9.1.info")
            .exists());
        assert!(native_cache
            .join("github.com!gin-gonic!gin/@v/v1.9.1.mod")
            .exists());
        assert!(native_cache
            .join("github.com!gin-gonic!gin/@v/v1.9.1.zip")
            .exists());
        assert!(native_cache
            .join("golang.org!x!text/@v/v0.3.7.info")
            .exists());
        assert!(native_cache
            .join("golang.org!x!text/@v/v0.3.7.mod")
            .exists());

        // Re-run: everything should be skipped (idempotent).
        let result2 = sync_cache(&war_cache, &native_cache).expect("second sync failed");
        assert_eq!(result2.stats.copied, 0);
        assert_eq!(
            result2.stats.skipped, 5,
            "second run must be fully idempotent"
        );
        assert_eq!(result2.stats.collisions, 0);
    }
}
