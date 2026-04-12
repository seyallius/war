//! Integration tests for vendor parser. Requires `go` installed and internet access.
//!
//! These tests require Go toolchain and internet access.
//! Run with: `cargo test -p war-go -- --ignored`

use std::{env, path::PathBuf, process::Command};
use tempfile::tempdir;
use tokio::runtime::Runtime;
use war_go::{init_project, vendor::parse_vendor_manifest};

// -------------------------------------------- Integration Tests --------------------------------------------

#[test]
#[ignore] // Requires internet + go CLI
fn test_parse_real_vendor_manifest() {
    if !go_installed() {
        eprintln!("Skipping test: go command not found");
        return;
    }

    let go_path = get_go_path();
    let go_path_str = go_path.to_str().expect("Invalid Go binary path");

    let rt = Runtime::new().unwrap();

    rt.block_on(async {
        let temp_dir = tempdir().unwrap();
        let project_name = "test-vendor-parse";
        let original_dir = env::current_dir().unwrap();

        env::set_current_dir(temp_dir.path()).unwrap();

        // Step 1: scaffold project
        let project_path = init_project(project_name)
            .await
            .expect("Failed to initialize project");

        // Step 2: fetch a real module (this runs go mod vendor internally)
        war_go::fetch_module_with_go_path(
            "github.com/gofiber/fiber/v2",
            &project_path,
            go_path_str,
        )
        .await
        .expect("Failed to fetch module");

        env::set_current_dir(original_dir).unwrap();

        // Step 3: parse the real vendor/modules.txt
        let modules =
            parse_vendor_manifest(&project_path).expect("Failed to parse vendor/modules.txt");

        // Step 4: assertions
        assert!(!modules.is_empty(), "Expected at least one vendored module");

        // The fetched module itself must appear
        let fiber = modules.iter().find(|m| m.path.contains("gofiber/fiber"));
        assert!(
            fiber.is_some(),
            "Expected gofiber/fiber in parsed modules, got: {:?}",
            modules.iter().map(|m| &m.path).collect::<Vec<_>>()
        );

        let fiber = fiber.unwrap();
        assert!(
            fiber.version.starts_with('v'),
            "Version should start with 'v', got: {}",
            fiber.version
        );
        assert!(
            fiber.explicit,
            "Directly fetched module should be marked explicit"
        );

        // Transitive deps should also be present
        // gofiber/fiber pulls in several deps (gofiber/utils, valyala/fasthttp, etc.)
        assert!(
            modules.len() > 1,
            "Expected transitive dependencies, got only {} module(s)",
            modules.len()
        );

        // Every module must have a valid path and version
        for module in &modules {
            assert!(
                module.path.contains('/'),
                "Module path should contain '/': {}",
                module.path
            );
            assert!(
                module.version.starts_with('v'),
                "Module version should start with 'v': {} {}",
                module.path,
                module.version
            );
            // Packages list should not be empty for explicit modules
            if module.explicit {
                assert!(
                    !module.packages.is_empty(),
                    "Explicit module {} should have at least one package",
                    module.path
                );
            }
        }

        eprintln!(
            "Successfully parsed {} modules from vendor/modules.txt",
            modules.len()
        );
    });
}

// --------------------------------------------- Internal Helpers ---------------------------------------------

fn find_go_binary() -> Option<PathBuf> {
    if let Ok(output) = Command::new("go").arg("version").output() {
        if output.status.success() {
            return Some(PathBuf::from("go"));
        }
    }

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

fn go_installed() -> bool {
    find_go_binary().is_some()
}

fn get_go_path() -> PathBuf {
    find_go_binary().expect("Go binary not found")
}
