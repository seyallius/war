//! Integration tests for pack module - archive Go module cache.
//!
//! These tests verify zip archive creation, filtering, and error handling.
//! Run with: `cargo test -p war-go pack_integration -- --ignored`

use std::{
    fs,
    path::{Path, PathBuf},
};
use tempfile::tempdir;
use war_go::pack_modules;
use zip::ZipArchive;

/// Helper to create a realistic Go module cache structure for testing.
fn create_mock_cache(cache_root: &Path, modules: &[(&str, &str)]) {
    for (module_path, version) in modules {
        let module_dir = cache_root.join(module_path.replace('/', "!")).join("@v");
        fs::create_dir_all(&module_dir).expect("Failed to create module cache dir");

        // Create .info file
        let info_content = format!(
            r#"{{
  "Version": "{}",
  "Time": "2024-01-01T00:00:00Z",
  "Origin": {{ "VCS": "git", "URL": "https://{}", "Hash": "{}" }}
}}"#,
            version, module_path, version
        );
        fs::write(module_dir.join(format!("{}.info", version)), info_content)
            .expect("Failed to write .info file");

        // Create .mod file
        let mod_content = format!("module {}\n\ngo 1.21\n", module_path);
        fs::write(module_dir.join(format!("{}.mod", version)), mod_content)
            .expect("Failed to write .mod file");

        // Create .zip file (minimal placeholder for testing)
        fs::write(
            module_dir.join(format!("{}.zip", version)),
            b"FAKE_ZIP_CONTENT",
        )
        .expect("Failed to write .zip file");
    }
}

#[test]
#[ignore] // Requires async runtime
fn test_pack_all_modules_no_filter() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let cache_dir = tempdir().unwrap();
        let output_dir = tempdir().unwrap();
        let output_file = output_dir.path().join("cache-archive.zip");

        // Create mock cache with 3 modules
        let modules = vec![
            ("github.com/gin-gonic/gin", "v1.9.1"),
            ("golang.org/x/net", "v0.10.0"),
            ("github.com/stretchr/testify", "v1.8.4"),
        ];
        create_mock_cache(cache_dir.path(), &modules);

        // Pack all modules (no filter)
        pack_modules(cache_dir.path(), &output_file, None)
            .await
            .expect("Failed to pack modules");

        // Verify archive exists
        assert!(output_file.exists(), "Output zip file not created");

        // Verify archive contents
        let file = fs::File::open(&output_file).unwrap();
        let mut zip = ZipArchive::new(file).unwrap();

        // Should contain all 9 files (3 modules × 3 files each)
        assert_eq!(zip.len(), 9, "Expected 9 files in archive");

        // Verify expected paths exist
        let expected_paths = [
            "github.com/gin-gonic/gin/@v/v1.9.1.info",
            "github.com/gin-gonic/gin/@v/v1.9.1.mod",
            "github.com/gin-gonic/gin/@v/v1.9.1.zip",
            "golang.org/x/net/@v/v0.10.0.info",
            "golang.org/x/net/@v/v0.10.0.mod",
            "golang.org/x/net/@v/v0.10.0.zip",
            "github.com/stretchr/testify/@v/v1.8.4.info",
            "github.com/stretchr/testify/@v/v1.8.4.mod",
            "github.com/stretchr/testify/@v/v1.8.4.zip",
        ];

        for expected in &expected_paths {
            assert!(
                zip.by_name(expected).is_ok(),
                "Expected file {} not found in zip",
                expected
            );
        }
    });
}

#[test]
#[ignore] // Requires async runtime
fn test_pack_modules_with_filter() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let cache_dir = tempdir().unwrap();
        let output_dir = tempdir().unwrap();
        let output_file = output_dir.path().join("filtered-archive.zip");

        // Create mock cache with multiple modules
        let modules = vec![
            ("github.com/gin-gonic/gin", "v1.9.1"),
            ("github.com/gin-gonic/gin", "v1.9.0"), // Multiple versions
            ("golang.org/x/net", "v0.10.0"),
        ];
        create_mock_cache(cache_dir.path(), &modules);

        // Filter: only include gin v1.9.1
        let filter = vec![("github.com/gin-gonic/gin".to_string(), "v1.9.1".to_string())];

        pack_modules(cache_dir.path(), &output_file, Some(filter))
            .await
            .expect("Failed to pack modules with filter");

        let file = fs::File::open(&output_file).unwrap();
        let mut zip = ZipArchive::new(file).unwrap();

        // Should only contain 3 files (gin v1.9.1's .info, .mod, .zip)
        assert_eq!(zip.len(), 3, "Expected only filtered files");

        // Verify gin v1.9.1 files exist
        assert!(zip
            .by_name("github.com/gin-gonic/gin/@v/v1.9.1.info")
            .is_ok());
        assert!(zip
            .by_name("github.com/gin-gonic/gin/@v/v1.9.1.mod")
            .is_ok());
        assert!(zip
            .by_name("github.com/gin-gonic/gin/@v/v1.9.1.zip")
            .is_ok());

        // Verify other files NOT included
        assert!(
            zip.by_name("github.com/gin-gonic/gin/@v/v1.9.0.info")
                .is_err(),
            "v1.9.0 should be filtered out"
        );
        assert!(
            zip.by_name("golang.org/x/net/@v/v0.10.0.info").is_err(),
            "net module should be filtered out"
        );
    });
}

#[test]
#[ignore] // Requires async runtime
fn test_pack_modules_with_empty_cache_returns_error() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let non_existent = PathBuf::from("/tmp/war-test-nonexistent-12345");
        let output_dir = tempdir().unwrap();
        let output_file = output_dir.path().join("empty.zip");

        let result = pack_modules(&non_existent, &output_file, None).await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            err.to_string().contains("Cache root does not exist"),
            "Wrong error message: {}",
            err
        );
    });
}

#[test]
#[ignore] // Requires async runtime
fn test_pack_modules_maintains_directory_structure() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let cache_dir = tempdir().unwrap();
        let output_dir = tempdir().unwrap();
        let output_file = output_dir.path().join("archive.zip");

        // Create nested cache structure with subdirectories
        let module_path = "github.com/gin-gonic/gin";
        let version = "v1.9.1";
        let module_dir = cache_dir
            .path()
            .join(module_path.replace('/', "!"))
            .join("@v");
        fs::create_dir_all(&module_dir).unwrap();
        fs::write(module_dir.join(format!("{}.info", version)), "test content").unwrap();
        fs::write(
            module_dir.join(format!("{}.mod", version)),
            "module content",
        )
        .unwrap();
        fs::write(module_dir.join(format!("{}.zip", version)), "zip content").unwrap();

        pack_modules(cache_dir.path(), &output_file, None)
            .await
            .expect("Failed to pack");

        let file = fs::File::open(&output_file).unwrap();
        let mut zip = ZipArchive::new(file).unwrap();

        // Verify directory entries exist (ZIP spec allows directories as entries)
        let _dir_path = "github.com/gin-gonic/gin/@v/";
        // Some ZIP writers add directory entries, some don't. Check that files are at correct paths.
        assert!(
            zip.by_name("github.com/gin-gonic/gin/@v/v1.9.1.info")
                .is_ok(),
            "File path structure incorrect"
        );
        assert!(
            zip.by_name("github.com/gin-gonic/gin/@v/v1.9.1.mod")
                .is_ok(),
            "File path structure incorrect"
        );
        assert!(
            zip.by_name("github.com/gin-gonic/gin/@v/v1.9.1.zip")
                .is_ok(),
            "File path structure incorrect"
        );
    });
}

#[test]
#[ignore] // Requires async runtime
fn test_pack_modules_handles_non_cache_files_gracefully() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let cache_dir = tempdir().unwrap();
        let output_dir = tempdir().unwrap();
        let output_file = output_dir.path().join("archive.zip");

        // Create a mix of valid cache files and unrelated files
        let module_dir = cache_dir.path().join("github.com!gin-gonic!gin").join("@v");
        fs::create_dir_all(&module_dir).unwrap();
        fs::write(module_dir.join("v1.9.1.info"), "info").unwrap();

        // Add unrelated file in cache root (should be ignored by filter logic)
        fs::write(cache_dir.path().join("README.md"), "unrelated").unwrap();

        pack_modules(cache_dir.path(), &output_file, None)
            .await
            .expect("Failed to pack with non-cache files");

        let file = fs::File::open(&output_file).unwrap();
        let mut zip = ZipArchive::new(file).unwrap();

        // Only the .info file inside @v/ should be included
        // (unrelated files outside @v/ are skipped by matches_filter)
        assert!(zip
            .by_name("github.com/gin-gonic/gin/@v/v1.9.1.info")
            .is_ok());
    });
}

#[test]
#[ignore] // Requires async runtime
fn test_pack_modules_with_multiple_versions_same_module() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let cache_dir = tempdir().unwrap();
        let output_dir = tempdir().unwrap();
        let output_file = output_dir.path().join("archive.zip");

        let module_path = "github.com/gin-gonic/gin";
        let versions = ["v1.9.0", "v1.9.1", "v1.10.0"];
        let module_dir = cache_dir
            .path()
            .join(module_path.replace('/', "!"))
            .join("@v");
        fs::create_dir_all(&module_dir).unwrap();

        for version in &versions {
            fs::write(
                module_dir.join(format!("{}.info", version)),
                format!("{} info", version),
            )
            .unwrap();
            fs::write(
                module_dir.join(format!("{}.mod", version)),
                format!("{} mod", version),
            )
            .unwrap();
            fs::write(
                module_dir.join(format!("{}.zip", version)),
                format!("{} zip", version),
            )
            .unwrap();
        }

        pack_modules(cache_dir.path(), &output_file, None)
            .await
            .expect("Failed to pack");

        let file = fs::File::open(&output_file).unwrap();
        let mut zip = ZipArchive::new(file).unwrap();

        // Should have 9 files (3 versions × 3 files)
        assert_eq!(zip.len(), 9);

        for version in &versions {
            assert!(zip
                .by_name(&format!("github.com/gin-gonic/gin/@v/{}.info", version))
                .is_ok());
            assert!(zip
                .by_name(&format!("github.com/gin-gonic/gin/@v/{}.mod", version))
                .is_ok());
            assert!(zip
                .by_name(&format!("github.com/gin-gonic/gin/@v/{}.zip", version))
                .is_ok());
        }
    });
}
