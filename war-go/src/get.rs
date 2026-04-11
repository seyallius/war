//! get - Fetch a Go module, auto-import it, and vendor dependencies.
//!
//! Wraps `go get`, appends blank import to main.go, runs `go mod tidy`,
//! and executes `go mod vendor` to capture full dependency graph.

use std::{fs, path::PathBuf, str};
use tokio::process::Command;
use war_core::WarError;

// -------------------------------------------- Public API --------------------------------------------

/// Fetch a module, inject blank import, tidy, and vendor dependencies.
///
/// module_spec: format "github.com/user/repo[@v1.2.3]"
/// project_root: path to the Go project containing go.mod and main.go
pub async fn fetch_module(module_spec: &str, project_root: &PathBuf) -> Result<(), WarError> {
    fetch_module_with_go_path(module_spec, project_root, "go").await
}

/// Fetch a module with a specific Go binary path.
/// Useful for tests or when Go is in a non-standard location.
pub async fn fetch_module_with_go_path(
    module_spec: &str,
    project_root: &PathBuf,
    go_binary: &str,
) -> Result<(), WarError> {
    // 1. Run `go get <module>`
    run_go_command(project_root, go_binary, &["get", module_spec]).await?;

    // 2. Inject `_ "<module_path>"` into main.go
    let module_path = module_spec.split('@').next().unwrap_or(module_spec);
    append_blank_import(module_path, project_root).await?;

    // 3. Run `go mod tidy`
    run_go_command(project_root, go_binary, &["mod", "tidy"]).await?;

    // 4. Run `go mod vendor`
    run_go_command(project_root, go_binary, &["mod", "vendor"]).await?;

    Ok(())
}

// -------------------------------------------- Internal Helpers --------------------------------------------

/// Execute a Go command asynchronously in the project directory.
async fn run_go_command(
    project_root: &PathBuf,
    go_binary: &str,
    args: &[&str],
) -> Result<(), WarError> {
    let command_str = format!("{} {}", go_binary, args.join(" "));

    let output = Command::new(go_binary)
        .args(args)
        .current_dir(project_root)
        .output()
        .await
        .map_err(|e| WarError::GoCommandFailed {
            command: command_str.clone(),
            stderr: format!("Failed to spawn process: {}", e),
            exit_code: -1,
        })?;

    if !output.status.success() {
        let stderr = str::from_utf8(&output.stderr)
            .unwrap_or("Failed to decode stderr")
            .to_string();
        let exit_code = output.status.code().unwrap_or(-1);
        return Err(WarError::GoCommandFailed {
            command: command_str,
            stderr,
            exit_code,
        });
    }

    Ok(())
}

/// Append `_ "<module>"` to the import block in main.go.
async fn append_blank_import(module_path: &str, project_root: &PathBuf) -> Result<(), WarError> {
    let main_go_path = project_root.join("main.go");
    let content = fs::read_to_string(&main_go_path).map_err(|e| WarError::CacheWriteError {
        module: module_path.to_string(),
        source: e,
    })?;

    let placeholder = "// _ \"github.com/example/module\"";
    let import_line = format!("\t_ \"{}\"", module_path);

    let updated_content = if content.contains(placeholder) {
        // Replace the scaffolded placeholder comment
        content.replace(placeholder, import_line.as_str())
    } else {
        // Fallback: insert before the closing `)` of the import block
        let import_start = content
            .find("import (")
            .ok_or_else(|| WarError::VendorParseError {
                path: main_go_path.clone(),
                reason: "Missing 'import (' block in main.go".to_string(),
            })?;

        let after_import = &content[import_start..];
        if let Some(last_paren) = after_import.rfind(')') {
            let insert_idx = import_start + last_paren;
            let mut updated = content.clone();
            updated.insert_str(insert_idx, &format!("\n{}", import_line));
            updated
        } else {
            return Err(WarError::VendorParseError {
                path: main_go_path,
                reason: "Could not locate closing ')' in import block".to_string(),
            });
        }
    };

    fs::write(&main_go_path, updated_content).map_err(|e| WarError::CacheWriteError {
        module: module_path.to_string(),
        source: e,
    })?;

    Ok(())
}
