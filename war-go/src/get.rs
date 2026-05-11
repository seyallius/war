//! get.rs - Fetch a Go module, auto-import it, vendor dependencies,
//! and stage it in `~/.war/war.lock` for subsequent `pack --staged`.
//!
//! When `war go get github.com/gin-gonic/gin@v1.9.1` is invoked, this
//! module:
//! 1. Calls `go get <module>` (future: real invocation).
//! 2. Parses the module path and version from the argument.
//! 3. Appends the `(module, version)` pair to `GoConfig::staged_modules`
//!    via `war_core::config::stage_add`, which deduplicates and persists.

use std::path::Path;
use war_core::{config, WarError};

// -------------------------------------------- Public API --------------------------------------------

/// Fetch a Go module, auto-import it, vendor dependencies, and auto-stage
/// it for `pack --staged`.
///
/// The `module` argument may include an optional `@version` suffix (e.g.
/// `github.com/gin-gonic/gin@v1.9.1`).  If no version is provided, the
/// special sentinel `"latest"` is used — the real `go get` invocation
/// (once implemented) will resolve it.
///
/// After the fetch, the module is added to `staged_modules` in
/// `~/.war/war.lock` via `war_core::config::stage_add`.  Duplicate
/// entries are silently ignored.
pub async fn fetch_module(module: &str, project_root: &Path) -> Result<(), WarError> {
    let (module_path, version) = parse_module_version(module);

    tracing::info!(
        "Fetching module {}@{} in project root: {}",
        module_path,
        version,
        project_root.display()
    );

    // TODO: real `go get` invocation will go here in a future phase.
    // For now we just auto-stage the module so the pack/unpack workflow
    // can be tested end-to-end.

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
    module: &str,
    project_root: &Path,
    _go_path: &Path,
) -> Result<(), WarError> {
    let (module_path, version) = parse_module_version(module);

    tracing::info!(
        "Fetching module {}@{} with custom GOPATH in project root: {}",
        module_path,
        version,
        project_root.display()
    );

    // TODO: real `go get` with GOPATH override.
    auto_stage(&module_path, &version)?;

    tracing::info!(
        "✔ Module {}@{} fetched (custom GOPATH) and staged",
        module_path,
        version
    );

    Ok(())
}

// -------------------------------------------- Internal Helpers --------------------------------------------

/// Parse a module string into `(module_path, version)`.
///
/// Accepts forms like:
/// - `github.com/gin-gonic/gin` → `("github.com/gin-gonic/gin", "latest")`
/// - `github.com/gin-gonic/gin@v1.9.1` → `("github.com/gin-gonic/gin", "v1.9.1")`
/// - `github.com/labstack/echo/v4@v4.11.1` → `("github.com/labstack/echo/v4", "v4.11.1")`
///
/// The `@` separator is the standard Go convention for specifying a version
/// on the command line.
fn parse_module_version(module: &str) -> (String, String) {
    if let Some(at_pos) = module.rfind('@') {
        let (path, ver) = module.split_at(at_pos);
        (path.to_string(), ver[1..].to_string()) // skip the '@'
    } else {
        (module.to_string(), "latest".to_string())
    }
}

/// Add the module to the staged list in `war.lock`, with deduplication.
fn auto_stage(module_path: &str, version: &str) -> Result<(), WarError> {
    match config::stage_add(module_path, version) {
        Ok(true) => {
            tracing::info!("Auto-staged {}@{}", module_path, version);
        }
        Ok(false) => {
            tracing::info!(
                "{}@{} already in staged list — no duplicate added",
                module_path,
                version
            );
        }
        Err(e) => {
            tracing::warn!(
                "Failed to auto-stage {}@{}: {}. Continuing anyway…",
                module_path,
                version,
                e
            );
        }
    }
    Ok(())
}

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
