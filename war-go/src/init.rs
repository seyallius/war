//! init - Scaffold a minimal Go project with go.mod and main.go.
//!
//! Provides the foundation for war go init: creates directory, writes go.mod,
//! and sets up a main.go with a blank import block ready for war go get.

use std::{fs, path::PathBuf};
use war_core::WarError;

// -------------------------------------------- Public API --------------------------------------------

/// Initialize a new Go project directory with go.mod and a minimal main.go.
///
/// Returns the absolute path to the created project directory on success.
/// The main.go includes a blank import block ready for `war go get` to append to.
pub fn init_project(name: &str) -> Result<PathBuf, WarError> {
    let project_path = PathBuf::from(name);

    // Create project directory (and parents if needed)
    fs::create_dir_all(&project_path).map_err(|e| WarError::CacheWriteError {
        //todo: probably not the correct error variant?
        module: name.to_string(),
        source: e,
    })?;

    // Write go.mod
    let go_mod_content = format!("module {name}\n\ngo 1.25.1\n");
    let go_mod_path = project_path.join("go.mod");
    fs::write(&go_mod_path, go_mod_content).map_err(|e| WarError::CacheWriteError {
        //todo: probably not the correct error variant?
        module: name.to_string(),
        source: e,
    })?;

    // Write main.go with blank import block
    let main_go_content = r#"package main

// Blank import block — `war go get` will append modules here.
import (
	// _ "github.com/example/module"
)

func main() {
	// TODO: Add your application logic here.
}
"#;
    let main_go_path = project_path.join("main.go");
    fs::write(&main_go_path, main_go_content).map_err(|e| WarError::CacheWriteError {
        module: name.to_string(),
        source: e,
    })?;

    // Return absolute path for downstream use
    project_path
        .canonicalize()
        .map_err(|e| WarError::ConfigError {
            source: Box::new(e),
        })
}

// -------------------------------------------- Private Helper Functions --------------------------------------------

/// Internal helper: run `go fmt ./...` on the project directory.
///
/// Stubbed for Phase 1 — will be implemented with tokio::process in next step.
#[allow(dead_code)]
async fn format_project(_project_path: &PathBuf) -> Result<(), WarError> {
    // TODO: Implement with tokio::process::Command in Phase 1b
    Ok(())
}
