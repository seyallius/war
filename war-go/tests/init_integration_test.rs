//! Integration tests for init module using isolated temp directories.

use std::{env, fs};
use tempfile::tempdir;
use war_go::init_project;

/// Test that init_project creates a valid Go project structure
#[test]
fn test_init_project_creates_expected_files() {
    let temp_dir = tempdir().unwrap();
    let project_name = "test-project";
    let _project_path = temp_dir.path().join(project_name);

    // Run init in isolated temp directory
    let original_dir = env::current_dir().unwrap();
    env::set_current_dir(temp_dir.path()).unwrap();

    let result = init_project(project_name);
    env::set_current_dir(original_dir).unwrap();

    assert!(result.is_ok());
    let created_path = result.unwrap();

    // Verify go.mod exists and has correct content
    let go_mod = fs::read_to_string(created_path.join("go.mod")).unwrap();
    assert!(go_mod.starts_with("module test-project"));
    assert!(go_mod.contains("go 1.22"));

    // Verify main.go exists and has blank import block
    let main_go = fs::read_to_string(created_path.join("main.go")).unwrap();
    assert!(main_go.contains("package main"));
    assert!(main_go.contains("import ("));
    assert!(main_go.contains("// _ \"github.com/example/module\""));
}
