//! Integration tests for get module. Requires `go` installed and internet access.
//!
//! These tests require Go toolchain and internet access.
//! Run with: `cargo test -p war-go -- --ignored`

use std::{env, fs, path::PathBuf, process::Command};
use tempfile::tempdir;
use tokio::runtime::Runtime;
use war_go::init_project;

// -------------------------------------------- Integration Tests --------------------------------------------

#[test]
#[ignore] // Requires internet + go CLI
fn test_fetch_module_downloads_and_vendors() {
    // Skip test if go is not installed
    if !go_installed() {
        eprintln!("Skipping test: go command not found");
        eprintln!("Please install Go from https://golang.org/dl/");
        return;
    }

    // Get the path to the go binary
    let go_path = get_go_path();
    let go_path_str = go_path.to_str().expect("Invalid Go binary path");

    // Debug: show go version
    if let Ok(output) = Command::new(&go_path).arg("version").output() {
        eprintln!("{}", String::from_utf8_lossy(&output.stdout));
    }

    // Create a tokio runtime for async test execution
    let rt = Runtime::new().unwrap();

    rt.block_on(async {
        let temp_dir = tempdir().unwrap();
        let project_name = "test-get-project";

        // Save original directory
        let original_dir = env::current_dir().unwrap();

        // Change to temp directory for the test
        env::set_current_dir(temp_dir.path()).unwrap();

        // Scaffold first - this creates the project in the temp directory
        let project_path = init_project(project_name)
            .await
            .expect("Failed to initialize project");

        // Fetch a real module - use the returned project path and go binary path
        war_go::fetch_module_with_go_path(
            "github.com/gofiber/fiber/v3",
            &project_path,
            go_path_str,
        )
        .await
        .expect("Failed to fetch module");

        // Restore original directory
        env::set_current_dir(original_dir).unwrap();

        // Verify vendor directory was created
        assert!(
            project_path.join("vendor").exists(),
            "vendor directory not found"
        );
        assert!(
            project_path.join("vendor/modules.txt").exists(),
            "vendor/modules.txt not found"
        );

        // Verify main.go was updated with the blank import
        let main_go =
            fs::read_to_string(project_path.join("main.go")).expect("Failed to read main.go");
        assert!(
            main_go.contains("_ \"github.com/gofiber/fiber/v3\""),
            "main.go does not contain expected import: {}",
            main_go
        );
    });
}

// --------------------------------------------- Internal Helpers ---------------------------------------------

/// Find the `go` binary in common installation locations
fn find_go_binary() -> Option<PathBuf> {
    // First try PATH
    if let Ok(output) = Command::new("go").arg("version").output() {
        if output.status.success() {
            return Some(PathBuf::from("go"));
        }
    }

    // Common installation paths for Go
    let go_home_path = format!("/home/{}/go/bin/go", env::var("USER").unwrap_or_default());
    let go_home_path2 = format!("/home/{}/.go/bin/go", env::var("USER").unwrap_or_default());
    let common_paths = vec![
        "/usr/local/go/bin/go",
        "/usr/bin/go",
        "/usr/lib/go/bin/go",
        go_home_path.as_str(),
        go_home_path2.as_str(),
    ];

    for path in common_paths {
        let path = PathBuf::from(path);
        if path.exists() {
            return Some(path);
        }
    }

    None
}

/// Check if `go` command is available
fn go_installed() -> bool {
    find_go_binary().is_some()
}

/// Get the path to the `go` binary
fn get_go_path() -> PathBuf {
    find_go_binary().expect("Go binary not found in common locations")
}
