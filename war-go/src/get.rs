//! get.rs - Fetch a Go module, auto-import it, and populate the war cache.
//!
//! When `war go get github.com/gin-gonic/gin@v1.9.1` is invoked, this module:
//! 1. Runs `go get <module>` in the project root to fetch + update go.mod.
//! 2. Runs `go mod download` to populate $GOMODCACHE with .info/.mod/.zip files.
//! 3. Copies downloaded modules from $GOMODCACHE → ~/.war/cache/go (war's private cache).
//! 4. Auto-stages the (module, version) pair in ~/.war/war.lock for pack --staged.

use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
};
use walkdir::WalkDir;
use war_core::{config, WarError};

// -------------------------------------------- Public API --------------------------------------------

/// Fetch a Go module, auto-import it, and populate the war cache.
///
/// The `module` argument may include an optional `@version` suffix.
/// After fetch, the module is added to `staged_modules` in `~/.war/war.lock`.
pub async fn fetch_module(module: &str, project_root: &Path) -> Result<(), WarError> {
    let (module_path, version) = parse_module_version(module);

    tracing::info!(
        "Fetching module {}@{} in project root: {}",
        module_path,
        version,
        project_root.display()
    );

    // 1. Run `go get <module>` to update go.mod/go.sum
    run_go_get(project_root, module)?;

    // 2. Run `go mod download` to populate $GOMODCACHE
    run_go_mod_download(project_root)?;

    // 3. Copy downloaded modules from native cache → war cache
    sync_downloaded_to_war_cache(&module_path, &version)?;

    // 4. Auto-stage for pack --staged
    auto_stage(&module_path, &version)?;

    tracing::info!(
        "✔ Module {}@{} fetched and staged (◕‿◕✿)",
        module_path,
        version
    );

    Ok(())
}

/// Fetch a Go module using a custom GOPATH, then auto-stage it.
///
/// Identical to `fetch_module` except the `GOPATH` environment variable
/// is overridden for the duration of the `go get` invocation.
pub async fn fetch_module_with_go_path(
    _module: &str,
    _project_root: &Path,
    _go_path: &Path,
) -> Result<(), WarError> {
    // let (module_path, version) = parse_module_version(module);
    //
    // tracing::info!(
    //     "Fetching module {}@{} with custom GOPATH in project root: {}",
    //     module_path,
    //     version,
    //     project_root.display()
    // );
    //
    // // TODO: real `go get` with GOPATH override.
    // auto_stage(&module_path, &version)?;
    //
    // tracing::info!(
    //     "✔ Module {}@{} fetched (custom GOPATH) and staged",
    //     module_path,
    //     version
    // );
    //
    // Ok(())
    unimplemented!("Implement fetching modules with GOPATH overrides.")
}

// -------------------------------------------- Internal Helpers --------------------------------------------

/// Parse module@version into (path, version). Defaults version to "latest".
fn parse_module_version(module: &str) -> (String, String) {
    if let Some(at) = module.rfind('@') {
        (module[..at].to_string(), module[at + 1..].to_string())
    } else {
        (module.to_string(), "latest".to_string())
    }
}

/// Run `go get <module>` in the project directory.
fn run_go_get(project_root: &Path, module: &str) -> Result<(), WarError> {
    //todo(go-get-async): allow async downloading output in std
    let output = Command::new("go")
        .arg("get")
        .arg(module)
        .current_dir(project_root)
        .output()
        .map_err(|e| WarError::GoCommandFailed {
            command: format!("go get {}", module),
            stderr: format!("Failed to spawn: {}", e),
            exit_code: -1,
        })?;

    if !output.status.success() {
        return Err(WarError::GoCommandFailed {
            command: format!("go get {}", module),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
            exit_code: output.status.code().unwrap_or(-1),
        });
    }
    Ok(())
}

/// Run `go mod download` to populate the native module cache.
fn run_go_mod_download(project_root: &Path) -> Result<(), WarError> {
    let output = Command::new("go")
        .arg("mod")
        .arg("download")
        .current_dir(project_root)
        .output()
        .map_err(|e| WarError::GoCommandFailed {
            command: "go mod download".into(),
            stderr: format!("Failed to spawn: {}", e),
            exit_code: -1,
        })?;

    if !output.status.success() {
        return Err(WarError::GoCommandFailed {
            command: "go mod download".into(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
            exit_code: output.status.code().unwrap_or(-1),
        });
    }
    Ok(())
}

/// Copy downloaded module files from $GOMODCACHE → ~/.war/cache/go.
///
/// Go's download cache uses the GOPROXY protocol layout:
/// - Path: <GOMODCACHE>/cache/download/<module_path>/@v/<version>.*
/// - Module path uses `/` separators (NOT `!` like the source cache)
/// - Files: .info, .mod, .zip, list
///
/// We copy these verbatim into war's cache so pack/unpack work transparently.
fn sync_downloaded_to_war_cache(module_path: &str, version: &str) -> Result<(), WarError> {
    let native_cache = resolve_gomodcache()?; // Returns .../cache/download
    let war_cache = config::get_config_path()?
        .parent()
        .ok_or_else(|| WarError::InvalidInput("Invalid config path".into()))?
        .join("cache")
        .join("go");

    fs::create_dir_all(&war_cache)?;

    let src_dir = native_cache.join(module_path).join("@v");
    if !src_dir.exists() {
        tracing::warn!(
            "Module {}@{} not found in native download cache at {}. \
            Try running `go mod download {}` manually.",
            module_path,
            version,
            src_dir.display(),
            module_path
        );
        return Ok(()); // Non-fatal: let user retry
    }

    // Copy all @v/* files (info/mod/zip/list) to `war cache`
    // War cache DOES use `!` separators, so we encode on the destination side.
    let encoded_module = module_path.replace('/', "!");
    let dst_dir = war_cache.join(&encoded_module).join("@v");
    fs::create_dir_all(&dst_dir)?;

    for entry in WalkDir::new(&src_dir).into_iter().filter_map(|e| e.ok()) {
        if entry.file_type().is_file() {
            let relative = entry
                .path()
                .strip_prefix(&src_dir)
                .map_err(|e| WarError::InvalidInput(format!("Path error: {}", e)))?;
            let dst = dst_dir.join(relative);
            fs::copy(entry.path(), &dst).map_err(|e| WarError::CacheWriteError {
                module: module_path.to_string(),
                source: e,
            })?;
        }
    }

    tracing::debug!(
        "Copied {}@{} from native cache → war cache",
        module_path,
        version
    );
    Ok(())
}

/// Resolve $GOMODCACHE's *download* subdirectory, where `go mod download` writes .info/.mod/.zip.
///
/// Go's module cache has two layouts:
/// - Source cache: ~/go/pkg/mod/github.com!gin-gonic!gin@v1.9.1/ (extracted source)
/// - Download cache: ~/go/pkg/mod/cache/download/github.com!gin-gonic!gin/@v/v1.9.1.* (proxy protocol)
///
/// We need the *download* cache because that's what `go mod download` populates,
/// and it's the format that `war go pack/unpack` expects (GOPROXY protocol layout).
fn resolve_gomodcache() -> Result<PathBuf, WarError> {
    let base = if let Ok(cache) = env::var("GOMODCACHE") {
        PathBuf::from(cache)
    } else {
        let home = dirs::home_dir()
            .ok_or_else(|| WarError::InvalidInput("Could not determine home directory".into()))?;
        home.join("go").join("pkg").join("mod")
    };
    // Return the *download* subdirectory, not the source cache root.
    Ok(base.join("cache").join("download"))
}

/// Add module to staged list in war.lock.
fn auto_stage(module_path: &str, version: &str) -> Result<(), WarError> {
    match config::stage_add(module_path, version) {
        Ok(_) => Ok(()),
        Err(e) => {
            tracing::warn!("Failed to auto-stage {}@{}: {}", module_path, version, e);
            Ok(()) // Non-fatal: continue anyway
        }
    }
}

// -------------------------------------------- Tests --------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_module_version_with_at() {
        let (module, version) = parse_module_version("github.com/gin-gonic/gin@v1.9.1");
        assert_eq!(module, "github.com/gin-gonic/gin");
        assert_eq!(version, "v1.9.1");
    }

    #[test]
    fn test_parse_module_version_with_subpath_and_at() {
        let (module, version) = parse_module_version("github.com/labstack/echo/v4@v4.11.1");
        assert_eq!(module, "github.com/labstack/echo/v4");
        assert_eq!(version, "v4.11.1");
    }

    #[test]
    fn test_parse_module_version_without_at() {
        let (module, version) = parse_module_version("github.com/gin-gonic/gin");
        assert_eq!(module, "github.com/gin-gonic/gin");
        assert_eq!(version, "latest");
    }

    #[test]
    fn test_parse_module_version_at_only_version() {
        // Edge case: @v1 with no path before it (unlikely but safe)
        let (module, version) = parse_module_version("@v1");
        assert_eq!(module, "");
        assert_eq!(version, "v1");
    }
}
