//! config - Management of war.lock configuration file (~/.war/war.lock).
//!
//! Handles reading, writing, and updating the TOML-based config that tracks
//! last-used vendor paths, sync timestamps, and language-specific settings.

use crate::{
    error::WarError,
    types::{GoConfig, RustConfig},
};
use serde::{Deserialize, Serialize};
use std::{fs, io, path::PathBuf};

/// Root configuration structure stored in ~/.war/war.lock.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WarConfig {
    /// Schema version for future migration support.
    pub schema_version: u32,
    /// Go-specific configuration section.
    pub go: Option<GoConfig>,
    /// Rust-specific configuration section (future).
    pub rust: Option<RustConfig>,
}

impl Default for WarConfig {
    fn default() -> Self {
        Self {
            schema_version: 1,
            go: None,
            rust: None,
        }
    }
}

// -------------------------------------------- Public API --------------------------------------------

/// Load war.lock from the default location (~/.war/war.lock).
///
/// Returns a default config if the file doesn't exist yet.
pub fn load_config() -> Result<WarConfig, WarError> {
    let config_path = get_config_path()?;

    if !config_path.exists() {
        return Ok(WarConfig::default());
    }

    let content = fs::read_to_string(&config_path).map_err(|e| WarError::ConfigError {
        source: Box::new(e),
    })?;

    toml::from_str(&content).map_err(|e| WarError::ConfigError {
        source: Box::new(e),
    })
}

/// Save the current config to ~/.war/war.lock.
///
/// Creates the ~/.war directory if it doesn't exist.
pub fn save_config(config: &WarConfig) -> Result<(), WarError> {
    let config_path = get_config_path()?;

    // Ensure parent directory exists
    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| WarError::ConfigError {
            source: Box::new(e),
        })?;
    }

    let content = toml::to_string_pretty(config).map_err(|e| WarError::ConfigError {
        source: Box::new(e),
    })?;

    // Atomic write: write to temp file, then rename
    let temp_path = config_path.with_extension("tmp");
    std::fs::write(&temp_path, content).map_err(|e| WarError::ConfigError {
        source: Box::new(e),
    })?;

    std::fs::rename(&temp_path, &config_path).map_err(|e| WarError::ConfigError {
        source: Box::new(e),
    })?;

    Ok(())
}

/// Get the absolute path to war.lock (~/.war/war.lock).
pub fn get_config_path() -> Result<PathBuf, WarError> {
    let home = dirs::home_dir().ok_or_else(|| WarError::ConfigError {
        source: Box::new(io::Error::new(
            io::ErrorKind::NotFound,
            "Could not determine home directory",
        )),
    })?;

    Ok(home.join(".war").join("war.lock"))
}

// -------------------------------------------- Internal --------------------------------------------

/// Internal helper: ensure ~/.war directory exists.
#[allow(dead_code)] //note: Currently unused but will be needed in future phases
fn ensure_war_dir() -> Result<PathBuf, WarError> {
    let war_dir = dirs::home_dir()
        .ok_or_else(|| WarError::ShellDetectionError)?
        .join(".war");

    fs::create_dir_all(&war_dir).map_err(|e| WarError::ConfigError {
        source: Box::new(e),
    })?;

    Ok(war_dir)
}
