//! get.rs - Fetch a Go module, auto-import it, and populate the war cache.
//!
//! When `war go get github.com/gin-gonic/gin@v1.9.1` is invoked, this module:
//! 1. Runs `go get <module>` in the project root to fetch + update go.mod.
//! 2. Runs `go mod download` to populate $GOMODCACHE with .info/.mod/.zip files.
//! 3. Copies downloaded modules from $GOMODCACHE → ~/.war/cache/go (war's private cache).
//! 4. Auto-stages the (module, version) pair in ~/.war/war.lock for pack --staged.
//!
//! ## Real-time output streaming
//!
//! Both `go get` and `go mod download` are spawned with piped stdout/stderr.
//! Each line produced by the child process is forwarded to `tracing::info!`
//! immediately, giving the user real-time visibility into download progress.
//! On failure, the captured stderr is still available in the `WarError::GoCommandFailed`
//! variant for structured error reporting.

use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Stdio,
};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use walkdir::WalkDir;
use war_core::{config, WarError};

// -------------------------------------------- Public API --------------------------------------------

/// Fetch a Go module, auto-import it, and populate the war cache.
///
/// The `module` argument may include an optional `@version` suffix.
/// After fetch, the module is added to `staged_modules` in `~/.war/war.lock`.
///
/// # Real-time output
///
/// All stdout and stderr from `go get` and `go mod download` are streamed
/// line-by-line to the logger as they arrive, so the user sees download
/// progress in real time instead of a silent wait followed by a burst of text.
pub async fn fetch_module(module: &str, project_root: &Path) -> Result<(), WarError> {
    let (module_path, version) = parse_module_version(module);

    tracing::info!(
        "Fetching module {}@{} in project root: {}",
        module_path,
        version,
        project_root.display()
    );

    // 1. Run `go get <module>` to update go.mod/go.sum
    run_go_get(project_root, module).await?;

    // 2. Run `go mod download` to populate $GOMODCACHE
    run_go_mod_download(project_root).await?;

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

/// Run a Go sub-command with real-time stdout/stderr streaming.
///
/// Spawns the child process with piped stdout and stderr, then reads both
/// streams line-by-line in a concurrent loop.  Each line from the child is
/// forwarded to `tracing::info!` immediately so the user sees output as it
/// is produced — no silent wait followed by a burst.
///
/// On success (exit code 0), returns `Ok(())`.  On non-zero exit, all
/// captured stderr is packed into a `WarError::GoCommandFailed` for
/// structured error reporting, matching the behaviour of the previous
/// buffered implementation.
///
/// # Why not inherit stdout/stderr directly?
///
/// `Stdio::inherit()` would also give real-time output, but it bypasses the
/// tracing subsystem entirely — `go get`'s output would be interleaved with
/// war's own log lines in an unstructured way, and we would lose the ability
/// to capture stderr for error reporting.  By piping and re-logging, we keep
/// everything inside the tracing pipeline and retain a copy for error messages.
async fn run_go_command_streaming(
    program: &str,
    args: &[&str],
    project_root: &Path,
    command_label: &str,
) -> Result<(), WarError> {
    let mut child = Command::new(program)
        .args(args)
        .current_dir(project_root)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| WarError::GoCommandFailed {
            command: command_label.to_string(),
            stderr: format!("Failed to spawn: {}", e),
            exit_code: -1,
        })?;

    // Take ownership of the piped streams before awaiting the child.
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| WarError::GoCommandFailed {
            command: command_label.to_string(),
            stderr: "Failed to capture stdout: pipe not available".into(),
            exit_code: -1,
        })?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| WarError::GoCommandFailed {
            command: command_label.to_string(),
            stderr: "Failed to capture stderr: pipe not available".into(),
            exit_code: -1,
        })?;

    // Wrap both streams in async_buf_read so we can iterate lines.
    let stdout_reader = BufReader::new(stdout);
    let stderr_reader = BufReader::new(stderr);

    // We must read from both streams concurrently to avoid deadlocking
    // when the child fills its stdout pipe buffer while we're reading stderr
    // (or vice versa).  Spawn a task for each stream and join them.
    let stdout_label = command_label.to_string();
    let stdout_handle = tokio::spawn(async move {
        let mut lines = stdout_reader.lines();
        loop {
            match lines.next_line().await {
                Ok(Some(line)) if !line.is_empty() => {
                    tracing::info!("  │ {}", line);
                }
                Ok(Some(_)) => {}  // skip empty lines
                Ok(None) => break, // EOF
                Err(e) => {
                    tracing::debug!("{} stdout read error: {}", stdout_label, e);
                    break;
                }
            }
        }
    });

    let stderr_label = command_label.to_string();
    let stderr_handle = tokio::spawn(async move {
        let mut lines = stderr_reader.lines();
        let mut captured = Vec::new();
        loop {
            match lines.next_line().await {
                Ok(Some(line)) if !line.is_empty() => {
                    tracing::info!("  │ {}", line);
                    captured.extend_from_slice(line.as_bytes());
                    captured.push(b'\n');
                }
                Ok(Some(_)) => {}
                Ok(None) => break,
                Err(e) => {
                    tracing::debug!("{} stderr read error: {}", stderr_label, e);
                    break;
                }
            }
        }
        captured
    });

    // Wait for both stream readers to finish (they exit on pipe EOF,
    // which happens when the child closes its stdout/stderr — usually on exit).
    let _ = stdout_handle.await;
    let captured_stderr = stderr_handle.await.unwrap_or_default();

    // Now wait for the child process itself to exit.
    let status = child.wait().await.map_err(|e| WarError::GoCommandFailed {
        command: command_label.to_string(),
        stderr: format!("Failed to wait for child: {}", e),
        exit_code: -1,
    })?;

    if !status.success() {
        let stderr_str = String::from_utf8_lossy(&captured_stderr).to_string();
        return Err(WarError::GoCommandFailed {
            command: command_label.to_string(),
            stderr: stderr_str,
            exit_code: status.code().unwrap_or(-1),
        });
    }

    Ok(())
}

/// Run `go get <module>` in the project directory with real-time output streaming.
///
/// Delegates to [`run_go_command_streaming`] so every line produced by
/// `go get` appears in the terminal as it is emitted — no silent wait.
async fn run_go_get(project_root: &Path, module: &str) -> Result<(), WarError> {
    run_go_command_streaming(
        "go",
        &["get", module],
        project_root,
        &format!("go get {}", module),
    )
    .await
}

/// Run `go mod download` to populate the native module cache with real-time
/// output streaming.
///
/// Delegates to [`run_go_command_streaming`] so download progress lines
/// (e.g. individual module fetches) are visible immediately.
async fn run_go_mod_download(project_root: &Path) -> Result<(), WarError> {
    run_go_command_streaming("go", &["mod", "download"], project_root, "go mod download").await
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
/// # Behavior
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

    // ── run_go_command_streaming tests ────────────────────────────────────

    /// Run a simple command via `run_go_command_streaming` and verify
    /// that it succeeds.  Uses `echo` (available on all POSIX systems) as a
    /// stand-in for `go` so the test doesn't require a Go toolchain.
    #[tokio::test]
    async fn test_run_go_command_streaming_success() {
        let tmp = tempfile::tempdir().unwrap();

        // `echo hello` always exits 0 and prints "hello" to stdout.
        let result = run_go_command_streaming("echo", &["hello"], tmp.path(), "echo hello").await;

        assert!(result.is_ok(), "expected Ok, got: {:?}", result);
    }

    /// Verify that a failing command returns the correct `WarError::GoCommandFailed`.
    ///
    /// Uses `false` (POSIX utility that always exits 1) to simulate a command
    /// that fails without producing meaningful stderr.
    #[tokio::test]
    async fn test_run_go_command_streaming_failure() {
        let tmp = tempfile::tempdir().unwrap();

        let result = run_go_command_streaming("false", &[], tmp.path(), "false").await;

        let err = result.expect_err("expected GoCommandFailed");
        match err {
            WarError::GoCommandFailed {
                command, exit_code, ..
            } => {
                assert_eq!(command, "false");
                assert_eq!(exit_code, 1);
            }
            other => panic!("expected GoCommandFailed, got: {:?}", other),
        }
    }

    /// Verify that a non-existent command returns `GoCommandFailed` with
    /// exit code -1 and a "Failed to spawn" message.
    #[tokio::test]
    async fn test_run_go_command_streaming_spawn_failure() {
        let tmp = tempfile::tempdir().unwrap();

        let result = run_go_command_streaming(
            "/nonexistent/binary/that/does/not/exist",
            &[],
            tmp.path(),
            "nonexistent",
        )
        .await;

        let err = result.expect_err("expected GoCommandFailed");
        match err {
            WarError::GoCommandFailed {
                command,
                stderr,
                exit_code,
            } => {
                assert_eq!(command, "nonexistent");
                assert_eq!(exit_code, -1);
                assert!(
                    stderr.contains("Failed to spawn"),
                    "stderr should mention spawn failure, got: {}",
                    stderr
                );
            }
            other => panic!("expected GoCommandFailed, got: {:?}", other),
        }
    }

    /// Verify that stderr output from a command is captured and reported
    /// in the error when the command fails.
    ///
    /// Uses `sh -c 'echo err >&2; exit 1'` to print to stderr and exit 1.
    #[tokio::test]
    async fn test_run_go_command_streaming_captures_stderr() {
        let tmp = tempfile::tempdir().unwrap();

        let result = run_go_command_streaming(
            "sh",
            &["-c", "echo err_msg >&2; exit 1"],
            tmp.path(),
            "stderr-test",
        )
        .await;

        let err = result.expect_err("expected GoCommandFailed");
        match err {
            WarError::GoCommandFailed { stderr, .. } => {
                assert!(
                    stderr.contains("err_msg"),
                    "stderr should contain 'err_msg', got: {}",
                    stderr
                );
            }
            other => panic!("expected GoCommandFailed, got: {:?}", other),
        }
    }

    /// Verify that multi-line stdout is streamed without loss.
    ///
    /// Uses `printf` to produce two lines and confirms the command succeeds.
    #[tokio::test]
    async fn test_run_go_command_streaming_multiline_stdout() {
        let tmp = tempfile::tempdir().unwrap();

        let result = run_go_command_streaming(
            "sh",
            &["-c", "printf 'line1\\nline2\\n'"],
            tmp.path(),
            "multiline-test",
        )
        .await;

        assert!(result.is_ok(), "expected Ok, got: {:?}", result);
    }

    /// Verify that both stdout and stderr are streamed concurrently for a
    /// command that writes to both before exiting.
    ///
    /// Uses `sh -c` to write to both streams, then exit 0.
    #[tokio::test]
    async fn test_run_go_command_streaming_stdout_and_stderr() {
        let tmp = tempfile::tempdir().unwrap();

        let result = run_go_command_streaming(
            "sh",
            &["-c", "echo out_msg; echo err_msg >&2"],
            tmp.path(),
            "dual-stream-test",
        )
        .await;

        assert!(result.is_ok(), "expected Ok, got: {:?}", result);
    }

    // ── resolve_gomodcache tests ──────────────────────────────────────────

    #[test]
    fn test_resolve_gomodcache_env_override() {
        // Temporarily set GOMODCACHE to verify it takes precedence.
        let original = env::var("GOMODCACHE").ok();
        env::set_var("GOMODCACHE", "/tmp/fake_gomodcache");

        let result = resolve_gomodcache();
        assert_eq!(
            result.unwrap(),
            PathBuf::from("/tmp/fake_gomodcache/cache/download")
        );

        // Restore original value.
        match original {
            Some(v) => env::set_var("GOMODCACHE", v),
            None => env::remove_var("GOMODCACHE"),
        }
    }

    #[test]
    fn test_resolve_gomodcache_default_path() {
        // Ensure GOMODCACHE is not set so the default path is used.
        let original = env::var("GOMODCACHE").ok();
        env::remove_var("GOMODCACHE");

        let result = resolve_gomodcache().expect("should resolve");
        let expected_suffix = PathBuf::from("go")
            .join("pkg")
            .join("mod")
            .join("cache")
            .join("download");
        assert!(
            result.ends_with(&expected_suffix),
            "expected path ending with {:?}, got: {:?}",
            expected_suffix,
            result
        );

        // Restore original value.
        if let Some(v) = original {
            env::set_var("GOMODCACHE", v);
        }
    }
}
