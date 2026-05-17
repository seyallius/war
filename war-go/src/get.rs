//! get.rs - Fetch a Go module, auto-import it, and populate the war cache.
//!
//! When `war go get github.com/gin-gonic/gin@v1.9.1` is invoked, this module:
//! 1. Runs `go get <module>` in the project root to fetch + update go.mod.
//! 2. Runs `go mod download` to populate $GOMODCACHE with .info/.mod/.zip files.
//! 3. Copies downloaded modules from $GOMODCACHE → ~/.war/cache/go (war's private cache).
//! 4. Appends a blank `_ "module/path"` import to `main.go` (if present).
//! 5. Auto-stages the (module, version) pair in ~/.war/war.lock for pack --staged.

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
/// After fetch, the module is added to `staged_modules` in `~/.war/war.lock`
/// and a blank `_ "module/path"` import is appended to `main.go` (if present).
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

    // 4. Append blank import to main.go (non-fatal — project may not have one)
    match update_main_imports(project_root, &module_path) {
        Ok(true) => tracing::info!("✔ Added blank import for '{}' to main.go", module_path),
        Ok(false) => tracing::debug!(
            "main.go already imports '{}' — skipping duplicate",
            module_path
        ),
        Err(e) => tracing::warn!(
            "⚠ Could not update main.go imports for '{}': {}",
            module_path,
            e
        ),
    }

    // 5. Auto-stage for pack --staged
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

/// Append a blank `_ "module_path"` import to `<project_root>/main.go`.
///
/// # Behaviour
///
/// - Looks for `main.go` in `project_root`.  If absent the function returns
///   `Ok(false)` silently (non-fatal — not every project has a `main.go`).
/// - Finds the first `import (` block and inserts a new import line immediately
///   before the closing `)`.
/// - The inserted line uses a blank identifier (`_`) so the build does not
///   fail even when the package is imported only for side-effects.
/// - Skips insertion if the import path is already present anywhere in the
///   file (exact substring match) to prevent duplicates on repeated `get` runs.
/// - Writes via an atomic `.war.tmp` / `rename` to protect against partial
///   writes on crash.
///
/// # Returns
///
/// - `Ok(true)`  — import was added.
/// - `Ok(false)` — file is absent, or import already present (idempotent).
/// - `Err(_)`    — I/O error or malformed `import` block (caller logs & continues).
fn update_main_imports(project_root: &Path, module_path: &str) -> Result<bool, WarError> {
    let main_go = project_root.join("main.go");
    if !main_go.exists() {
        tracing::debug!(
            "main.go not found at {} — skipping import injection",
            main_go.display()
        );
        return Ok(false);
    }

    let content = fs::read_to_string(&main_go).map_err(WarError::IOError)?;

    // Idempotency: skip if the import path is already present anywhere in
    // the file (covers both `_ "path"` and plain `"path"` forms).
    if content.contains(&format!("\"{}\"", module_path)) {
        tracing::debug!(
            "\"{}\" already present in main.go — skipping duplicate import",
            module_path
        );
        return Ok(false);
    }

    let updated = inject_import(&content, module_path).ok_or_else(|| {
        WarError::InvalidInput(format!(
            "main.go at {} has no `import (...)` block to inject into.\n\
             Add one manually: import (\n\t// imports here\n)",
            main_go.display()
        ))
    })?;

    // Atomic write: .war.tmp sidecar → rename
    let tmp_path = main_go.with_extension("go.war.tmp");
    fs::write(&tmp_path, &updated).map_err(WarError::IOError)?;
    fs::rename(&tmp_path, &main_go).map_err(|e| {
        let _ = fs::remove_file(&tmp_path);
        WarError::IOError(std::io::Error::new(
            e.kind(),
            format!(
                "Atomic rename failed: {} → {}: {}",
                tmp_path.display(),
                main_go.display(),
                e
            ),
        ))
    })?;

    Ok(true)
}

/// Insert `\t_ "module_path"` before the closing `)` of the first
/// `import (...)` block found in `source`.
///
/// Returns `None` if no `import (` block exists in `source`.
///
/// This is a pure function with no I/O, kept separate so it can be
/// exhaustively unit-tested without touching the filesystem.
fn inject_import(source: &str, module_path: &str) -> Option<String> {
    // Find the `import (` opening.  We accept leading whitespace on the line.
    let import_open = source.find("import (")?;

    // Scan forward from `import (` to find the matching closing `)`.
    // We look for the first `)` that appears on its own line (after the
    // opening paren), which is the canonical Go import block style.
    let search_from = import_open + "import (".len();
    let close_offset = source[search_from..].find("\n)")?;
    let close_pos = search_from + close_offset + 1; // position of the `)`

    let import_line = format!("\t_ \"{}\"\n", module_path);

    let mut result = String::with_capacity(source.len() + import_line.len());
    result.push_str(&source[..close_pos]);
    result.push_str(&import_line);
    result.push_str(&source[close_pos..]);

    Some(result)
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
    use std::fs;
    use tempfile::TempDir;

    // ── parse_module_version ────────────────────────────────────────────────

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

    // ── inject_import — pure unit tests (no I/O) ────────────────────────────

    #[test]
    fn test_inject_import_adds_line_before_closing_paren() {
        let source = r#"package main

import (
	// _ "github.com/example/module"
)

func main() {}
"#;
        let result = inject_import(source, "github.com/gin-gonic/gin").unwrap();
        assert!(
            result.contains("\t_ \"github.com/gin-gonic/gin\"\n"),
            "injected line must be present:\n{}",
            result
        );
        // The closing paren must still appear after the new import.
        let gin_pos = result.find("_ \"github.com/gin-gonic/gin\"").unwrap();
        let close_pos = result[gin_pos..].find(')').unwrap();
        assert!(
            close_pos > 0,
            "closing paren must come after the injected import"
        );
    }

    #[test]
    fn test_inject_import_preserves_existing_content() {
        let source = r#"package main

import (
	"fmt"
)

func main() {
	fmt.Println("hello")
}
"#;
        let result = inject_import(source, "github.com/gin-gonic/gin").unwrap();
        assert!(result.contains("\"fmt\""), "existing imports must be kept");
        assert!(result.contains("_ \"github.com/gin-gonic/gin\""));
    }

    #[test]
    fn test_inject_import_no_import_block_returns_none() {
        let source = r#"package main

func main() {}
"#;
        assert!(
            inject_import(source, "github.com/gin-gonic/gin").is_none(),
            "must return None when no import block exists"
        );
    }

    #[test]
    fn test_inject_import_empty_import_block() {
        let source = "package main\n\nimport (\n)\n\nfunc main() {}\n";
        let result = inject_import(source, "github.com/foo/bar").unwrap();
        assert!(result.contains("\t_ \"github.com/foo/bar\"\n"));
    }

    #[test]
    fn test_inject_import_multiple_existing_imports() {
        let source = r#"package main

import (
	"fmt"
	"os"
)

func main() {}
"#;
        let result = inject_import(source, "github.com/gin-gonic/gin").unwrap();
        assert!(result.contains("\"fmt\""));
        assert!(result.contains("\"os\""));
        assert!(result.contains("_ \"github.com/gin-gonic/gin\""));
        // The injected line should appear before the closing paren.
        let gin_pos = result.find("_ \"github.com/gin-gonic/gin\"").unwrap();
        let close_pos = result.rfind(')').unwrap();
        assert!(gin_pos < close_pos, "import must precede closing paren");
    }

    // ── update_main_imports — filesystem tests ──────────────────────────────

    #[test]
    fn test_update_main_imports_adds_import_to_main_go() {
        let tmp = TempDir::new().unwrap();
        let main_go = tmp.path().join("main.go");

        fs::write(
            &main_go,
            r#"package main

import (
	// _ "github.com/example/module"
)

func main() {}
"#,
        )
        .unwrap();

        let added = update_main_imports(tmp.path(), "github.com/gin-gonic/gin").unwrap();
        assert!(added, "should report import was added");

        let content = fs::read_to_string(&main_go).unwrap();
        assert!(
            content.contains("_ \"github.com/gin-gonic/gin\""),
            "import must appear in main.go:\n{}",
            content
        );
    }

    #[test]
    fn test_update_main_imports_idempotent_on_second_call() {
        let tmp = TempDir::new().unwrap();
        let main_go = tmp.path().join("main.go");

        fs::write(&main_go, "package main\n\nimport (\n)\n\nfunc main() {}\n").unwrap();

        let first = update_main_imports(tmp.path(), "github.com/gin-gonic/gin").unwrap();
        assert!(first, "first call must add the import");

        let second = update_main_imports(tmp.path(), "github.com/gin-gonic/gin").unwrap();
        assert!(!second, "second call must be a no-op (already present)");

        // Verify only one copy of the import exists.
        let content = fs::read_to_string(&main_go).unwrap();
        let count = content.matches("_ \"github.com/gin-gonic/gin\"").count();
        assert_eq!(count, 1, "import must appear exactly once:\n{}", content);
    }

    #[test]
    fn test_update_main_imports_no_main_go_returns_false() {
        let tmp = TempDir::new().unwrap();
        // No main.go created — function must return Ok(false) gracefully.
        let result = update_main_imports(tmp.path(), "github.com/gin-gonic/gin").unwrap();
        assert!(!result, "must return false when main.go is absent");
    }

    #[test]
    fn test_update_main_imports_no_partial_file_on_missing_import_block() {
        let tmp = TempDir::new().unwrap();
        let main_go = tmp.path().join("main.go");

        // A main.go with no import block at all.
        let original = "package main\n\nfunc main() {}\n";
        fs::write(&main_go, original).unwrap();

        // Should return Err because inject_import finds no block.
        let result = update_main_imports(tmp.path(), "github.com/gin-gonic/gin");
        assert!(result.is_err(), "must error when no import block exists");

        // Original file must be untouched (no .war.tmp sidecar left behind).
        let content = fs::read_to_string(&main_go).unwrap();
        assert_eq!(content, original, "original content must be preserved");

        let tmp_path = main_go.with_extension("go.war.tmp");
        assert!(
            !tmp_path.exists(),
            ".war.tmp sidecar must be cleaned up on failure"
        );
    }

    #[test]
    fn test_update_main_imports_multiple_modules_in_sequence() {
        let tmp = TempDir::new().unwrap();
        let main_go = tmp.path().join("main.go");

        fs::write(
            &main_go,
            "package main\n\nimport (\n\t// _ \"github.com/example/module\"\n)\n\nfunc main() {}\n",
        )
        .unwrap();

        update_main_imports(tmp.path(), "github.com/gin-gonic/gin").unwrap();
        update_main_imports(tmp.path(), "golang.org/x/text").unwrap();

        let content = fs::read_to_string(&main_go).unwrap();
        assert!(content.contains("_ \"github.com/gin-gonic/gin\""));
        assert!(content.contains("_ \"golang.org/x/text\""));

        // Import block must still be syntactically closed.
        assert!(content.contains(')'), "closing paren must remain");
    }

    #[test]
    fn test_update_main_imports_atomic_no_tmp_file_after_success() {
        let tmp = TempDir::new().unwrap();
        let main_go = tmp.path().join("main.go");

        fs::write(&main_go, "package main\n\nimport (\n)\n\nfunc main() {}\n").unwrap();

        update_main_imports(tmp.path(), "github.com/gin-gonic/gin").unwrap();

        let tmp_path = main_go.with_extension("go.war.tmp");
        assert!(
            !tmp_path.exists(),
            ".war.tmp must not remain after successful write"
        );
    }
}
