//! Integration tests for init module using isolated temp directories.

use std::{env, fs};
use tempfile::tempdir;
use tokio::runtime::Runtime;
use war_go::init_project;

/// Test that init_project creates a valid Go project structure and runs go fmt
#[test]
fn test_init_project_creates_expected_files() {
    // Create a tokio runtime for async test execution
    let rt = Runtime::new().unwrap();

    rt.block_on(async {
        let temp_dir = tempdir().unwrap();
        let project_name = "test-project";
        let _project_path = temp_dir.path().join(project_name);

        // Change to temp directory for the test
        let original_dir = env::current_dir().unwrap();
        env::set_current_dir(temp_dir.path()).unwrap();

        // Run init - this will create the project in the temp directory
        let result = init_project(project_name).await;

        // Restore original directory
        env::set_current_dir(original_dir).unwrap();

        assert!(result.is_ok(), "init_project failed: {:?}", result.err());
        let created_path = result.unwrap();

        // Verify go.mod exists and has correct content
        let go_mod_path = created_path.join("go.mod");
        assert!(
            go_mod_path.exists(),
            "go.mod not found at {:?}",
            go_mod_path
        );
        let go_mod = fs::read_to_string(go_mod_path).unwrap();
        assert!(go_mod.starts_with("module test-project"));
        assert!(
            go_mod.contains("go 1.22"),
            "Expected go 1.22, got: {}",
            go_mod
        );

        // Verify main.go exists and has blank import block
        let main_go_path = created_path.join("main.go");
        assert!(
            main_go_path.exists(),
            "main.go not found at {:?}",
            main_go_path
        );
        let main_go = fs::read_to_string(main_go_path).unwrap();
        assert!(main_go.contains("package main"));
        assert!(main_go.contains("import ("));
        assert!(main_go.contains("// _ \"github.com/example/module\""));
    });
}
