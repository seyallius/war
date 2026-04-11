//! Integration tests for config module using isolated temp directories.

use chrono::Utc;
use war_core::{
    config::{load_config, save_config, WarConfig},
    types::GoConfig,
};

/// Test that configuration can be saved and loaded correctly with isolated temp directory
#[test]
fn test_config_round_trip_with_temp_dir() {
    let temp_dir = tempfile::tempdir().unwrap();
    let original_home = std::env::var("HOME").ok();

    // Temporarily override HOME for test isolation
    std::env::set_var("HOME", temp_dir.path());

    let mut config = WarConfig::default();
    config.go = Some(GoConfig {
        last_vendor_path: Some("/test/vendor".into()),
        last_sync_timestamp: Some(Utc::now()),
        go_version: Some("1.22.2".into()),
    });

    save_config(&config).unwrap();
    let loaded = load_config().unwrap();

    assert_eq!(loaded.schema_version, 1);
    assert!(loaded.go.is_some());

    // Restore original HOME
    if let Some(home) = original_home {
        std::env::set_var("HOME", home);
    } else {
        std::env::remove_var("HOME");
    }
}
