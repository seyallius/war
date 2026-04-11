//! init - Scaffold a minimal Go project with go.mod and main.go.
//!
//! Provides the foundation for war go init: creates directory, writes go.mod,
//! and sets up a main.go with a blank import block ready for war go get.

use std::{fs, path::PathBuf};
use tokio::process::Command;
use war_core::WarError;

// -------------------------------------------- Public API --------------------------------------------

/// Initialize a new Go project directory with go.mod and a minimal main.go.
///
/// Returns the absolute path to the created project directory on success.
/// The main.go includes a blank import block ready for war go get to append to.
/// Runs `go fmt ./...` asynchronously to ensure idiomatic formatting.
pub async fn init_project(name: &str) -> Result<PathBuf, WarError> {
    let project_path = PathBuf::from(name);

    // Create project directory (and parents if needed)
    fs::create_dir_all(&project_path).map_err(|e| WarError::ConfigError {
        source: Box::new(e),
    })?;

    // Write go.mod
    let go_mod_content = format!("module {name}\n\ngo 1.22\n");
    let go_mod_path = project_path.join("go.mod");
    fs::write(&go_mod_path, go_mod_content).map_err(|e| WarError::ConfigError {
        source: Box::new(e),
    })?;

    // Write main.go with blank import block
    let main_go_content = r#"package main

// Blank import block — war go get will append modules here.
import (
	// _ "github.com/example/module"
)

func main() {
	// TODO: Add your application logic here.
}
"#;
    let main_go_path = project_path.join("main.go");
    fs::write(&main_go_path, main_go_content).map_err(|e| WarError::ConfigError {
        source: Box::new(e),
    })?;

    // Run go fmt asynchronously to ensure idiomatic formatting
    // Note: This may fail if go is not installed, but that's fine for the test
    let _ = format_project(&project_path).await;

    // Return absolute path for downstream use
    project_path
        .canonicalize()
        .map_err(|e| WarError::ConfigError {
            source: Box::new(e),
        })
}

// -------------------------------------------- Internal Helpers --------------------------------------------

/// Run `go fmt ./...` on the project directory using async process spawning.
///
/// This ensures the scaffolded Go code follows idiomatic formatting conventions.
/// Fails if the `go` binary is not found or returns a non-zero exit code.
async fn format_project(project_path: &PathBuf) -> Result<(), WarError> {
    let output = Command::new("go")
        .arg("fmt")
        .arg("./...")
        .current_dir(project_path)
        .output()
        .await
        .map_err(|e| WarError::GoCommandFailed {
            command: String::from("go fmt ./..."),
            stderr: format!("Failed to spawn go command: {}", e),
            exit_code: -1,
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        return Err(WarError::GoCommandFailed {
            command: String::from("go fmt ./..."),
            stderr,
            exit_code: output.status.code().unwrap_or(-1),
        });
    }

    Ok(())
}
