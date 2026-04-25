//! pack.rs - Archive Go module cache for airgap transfer.

use std::{fs::File, io, io::BufWriter, path::Path};
use walkdir::WalkDir;
use war_core::error::WarError;
use zip::{write::FileOptions, ZipWriter};

// -------------------------------------------- Public API --------------------------------------------

/// Pack Go module cache into a zip archive.
pub async fn pack_modules(
    cache_root: &Path,
    output: &Path,
    staged_filter: Option<Vec<(String, String)>>,
) -> Result<(), WarError> {
    if !cache_root.exists() {
        return Err(WarError::InvalidInput("Cache root does not exist".into()));
    }

    let file = File::create(output).map_err(|e| WarError::IOError(e))?;
    let writer = BufWriter::new(file);
    let mut zip = ZipWriter::new(writer);

    let options: FileOptions<'_, ()> = FileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .unix_permissions(0o755);

    for entry in WalkDir::new(cache_root) {
        let entry = entry.map_err(|e| WarError::IOError(e.into()))?;
        let path = entry.path();

        if path.is_file() {
            if let Some(filter) = &staged_filter {
                if !matches_filter(cache_root, path, filter)? {
                    continue;
                }
            }

            let relative = path.strip_prefix(cache_root).map_err(|e| {
                WarError::InvalidInput(format!(
                    "Path {} not under cache root {}: {}",
                    path.display(),
                    cache_root.display(),
                    e
                ))
            })?;
            // zip.start_file(relative.to_string_lossy(), options)
            let zip_path = normalize_cache_path(relative);
            zip.start_file(zip_path, options)
                .map_err(|e| WarError::ZipCreationError {
                    module: entry.file_name().to_string_lossy().to_string(),
                    source: e,
                })?;
            let mut f = File::open(path)?;
            io::copy(&mut f, &mut zip)?; //fixme: should this be tokio's?
        }
    }

    zip.finish().map_err(|e| WarError::ZipCreationError {
        module: cache_root.display().to_string(),
        source: e,
    })?;
    Ok(())
}

// --------------------------------------------- Internal Helpers ---------------------------------------------

/// Check if file belongs to staged module list.
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

    // find "@v" component
    let v_idx = parts
        .iter()
        .position(|c| c.as_os_str() == "@v")
        .ok_or_else(|| WarError::InvalidInput("Invalid cache path (missing @v)".into()))?;

    if v_idx + 1 >= parts.len() {
        return Ok(false);
    }

    // module path = everything before "@v"
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

    Ok(filter.contains(&(module, version)))
}

/// Convert go module cache paths (with '!') to canonical module paths (with '/').
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_pack_all_modules() {
        let dir = tempdir().unwrap();
        let cache = dir.path().join("cache");
        fs::create_dir_all(cache.join("github.com!test/@v")).unwrap();
        fs::write(cache.join("github.com!test/@v/v1.0.0.info"), "test").unwrap();

        let archive = dir.path().join("out.zip");

        pack_modules(&cache, &archive, None).await.unwrap();

        assert!(archive.exists());
    }

    #[test]
    fn normalize_cache_path_rewrites_bangs_in_first_component() {
        let p = Path::new("github.com!gin-gonic!gin/@v/v1.9.1.info");
        assert_eq!(
            normalize_cache_path(p),
            "github.com/gin-gonic/gin/@v/v1.9.1.info"
        );
    }

    #[test]
    fn normalize_cache_path_leaves_regular_paths_unchanged() {
        let p = Path::new("golang.org/x/text/@v/v0.3.7.info");
        assert_eq!(normalize_cache_path(p), "golang.org/x/text/@v/v0.3.7.info");
    }
}
