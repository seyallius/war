//! config.rs - Management of war.lock configuration file (~/.war/war.lock).
//!
//! Provides load/save helpers for the `WarConfig` TOML file, plus
//! convenience functions for manipulating the staged-modules list with
//! automatic deduplication and atomic persistence.

use crate::{
    error::WarError,
    types::{GoConfig, RustConfig, StagedModule},
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
            go: Some(GoConfig::default()),
            rust: None,
        }
    }
}

// -------------------------------------------- Public API --------------------------------------------

/// Load the war configuration from `~/.war/war.lock`.
///
/// If the file does not exist, returns a default `WarConfig` with an
/// empty `go.staged_modules` list.  If the file exists but the `go`
/// section is absent, it is initialised with defaults.
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

/// Save the war configuration to `~/.war/war.lock` atomically.
///
/// Writes to a `.tmp` sidecar first, then `fs::rename`s into place to
/// avoid partial writes on crash.  This is the same strategy used by
/// `unpack_modules` for file extraction.
pub fn save_config(config: &WarConfig) -> Result<(), WarError> {
    let config_path = get_config_path()?;

    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| WarError::ConfigError {
            source: Box::new(e),
        })?;
    }

    let content = toml::to_string_pretty(config).map_err(|e| WarError::ConfigError {
        source: Box::new(e),
    })?;

    let temp_path = config_path.with_extension("tmp");
    std::fs::write(&temp_path, content).map_err(|e| WarError::ConfigError {
        source: Box::new(e),
    })?;

    std::fs::rename(&temp_path, &config_path).map_err(|e| WarError::ConfigError {
        source: Box::new(e),
    })?;

    Ok(())
}

/// Return the path to `~/.war/war.lock`.
pub fn get_config_path() -> Result<PathBuf, WarError> {
    let home = dirs::home_dir().ok_or_else(|| WarError::ConfigError {
        source: Box::new(io::Error::new(
            io::ErrorKind::NotFound,
            "Could not determine home directory",
        )),
    })?;

    Ok(home.join(".war").join("war.lock"))
}

/// Add a module to the staged list and persist to `war.lock`.
///
/// Deduplicates by `(module, version)` — if the pair already exists the
/// function returns `Ok(false)` without writing.  On successful addition
/// the config is saved atomically and `Ok(true)` is returned.
pub fn stage_add(module: &str, version: &str) -> Result<bool, WarError> {
    let mut config = load_config()?;
    let go = ensure_go_config(&mut config);

    let candidate = StagedModule {
        module: module.to_string(),
        version: version.to_string(),
    };

    if go.staged_modules.contains(&candidate) {
        tracing::info!(
            "Module {}@{} already staged — skipping (◕‿◕)",
            module,
            version
        );
        return Ok(false);
    }

    go.staged_modules.push(candidate);
    go.staged_modules.sort(); // keep deterministic order for TOML diffs
    save_config(&config)?;
    tracing::info!("✔ Staged {}@{}", module, version);
    Ok(true)
}

/// Remove a module from the staged list and persist to `war.lock`.
///
/// Returns `Ok(true)` if the module was found and removed, `Ok(false)`
/// if it was not in the list.
pub fn stage_remove(module: &str, version: &str) -> Result<bool, WarError> {
    let mut config = load_config()?;
    let go = ensure_go_config(&mut config);

    let original_len = go.staged_modules.len();
    go.staged_modules
        .retain(|sm| !(sm.module == module && sm.version == version));

    if go.staged_modules.len() == original_len {
        tracing::info!("Module {}@{} not found in staged list", module, version);
        return Ok(false);
    }

    save_config(&config)?;
    tracing::info!("✔ Unstaged {}@{}", module, version);
    Ok(true)
}

/// Clear all staged modules and persist to `war.lock`.
pub fn stage_clear() -> Result<(), WarError> {
    let mut config = load_config()?;
    let go = ensure_go_config(&mut config);
    go.staged_modules.clear();
    save_config(&config)?;
    tracing::info!("✔ Staged modules cleared");
    Ok(())
}

/// Return the current staged-modules list from `war.lock`.
pub fn stage_list() -> Result<Vec<StagedModule>, WarError> {
    let config = load_config()?;
    Ok(config.go.map(|g| g.staged_modules).unwrap_or_default())
}

/// Return the staged list as `(module, version)` string pairs for use
/// as a filter in `pack_modules` / `unpack_modules_with_opts`.
pub fn staged_filter_pairs() -> Result<Vec<(String, String)>, WarError> {
    let list = stage_list()?;
    Ok(list.into_iter().map(|sm| (sm.module, sm.version)).collect())
}

// -------------------------------------------- Internal Helpers --------------------------------------------

/// Ensure the `go` section of `WarConfig` is initialised.  If it is
/// `None`, a default `GoConfig` (with empty `staged_modules`) is
/// inserted and a mutable reference is returned.
fn ensure_go_config(config: &mut WarConfig) -> &mut GoConfig {
    if config.go.is_none() {
        config.go = Some(GoConfig::default());
    }
    config.go.as_mut().expect("just initialised go config")
}

/// Ensure the `~/.war` directory exists, returning its path.
#[allow(dead_code)]
fn ensure_war_dir() -> Result<PathBuf, WarError> {
    let war_dir = dirs::home_dir()
        .ok_or_else(|| WarError::ShellDetectionError)?
        .join(".war");

    fs::create_dir_all(&war_dir).map_err(|e| WarError::ConfigError {
        source: Box::new(e),
    })?;

    Ok(war_dir)
}

#[cfg(test)]
mod config_tests {
    use super::*;

    // Override config path for tests so we don't clobber the real war.lock.
    #[allow(dead_code)]
    fn with_test_config<F, R>(test_fn: F) -> R
    where
        F: FnOnce() -> R,
    {
        // We can't easily override get_config_path in unit tests, so
        // we test via the in-memory GoConfig::default instead.
        test_fn()
    }

    #[test]
    fn test_go_config_default_has_empty_staged_modules() {
        let go = GoConfig::default();
        assert!(go.staged_modules.is_empty());
    }

    #[test]
    fn test_staged_module_ordering() {
        let a = StagedModule {
            module: "a.com/mod".into(),
            version: "v1.0.0".into(),
        };
        let b = StagedModule {
            module: "b.com/mod".into(),
            version: "v2.0.0".into(),
        };
        assert!(a < b);
    }

    #[test]
    fn test_staged_module_equality_and_dedup() {
        let a = StagedModule {
            module: "github.com/gin-gonic/gin".into(),
            version: "v1.9.1".into(),
        };
        let b = StagedModule {
            module: "github.com/gin-gonic/gin".into(),
            version: "v1.9.1".into(),
        };
        assert_eq!(a, b);

        let mut set = std::collections::HashSet::new();
        assert!(set.insert(a.clone()));
        assert!(!set.insert(b)); // duplicate rejected
    }

    #[test]
    fn test_war_config_default_includes_go() {
        let config = WarConfig::default();
        assert!(config.go.is_some());
        assert!(config.go.unwrap().staged_modules.is_empty());
    }

    #[test]
    fn test_toml_round_trip() {
        let mut config = WarConfig::default();
        let go = config.go.as_mut().unwrap();
        go.staged_modules.push(StagedModule {
            module: "github.com/gin-gonic/gin".into(),
            version: "v1.9.1".into(),
        });
        go.staged_modules.push(StagedModule {
            module: "golang.org/x/text".into(),
            version: "v0.3.7".into(),
        });

        let toml_str = toml::to_string_pretty(&config).expect("serialize failed");
        let deserialized: WarConfig = toml::from_str(&toml_str).expect("deserialize failed");

        assert_eq!(deserialized.go.unwrap().staged_modules.len(), 2);
    }

    #[test]
    fn test_ensure_go_config_creates_default() {
        let mut config = WarConfig {
            schema_version: 1,
            go: None,
            rust: None,
        };
        let go = ensure_go_config(&mut config);
        assert!(go.staged_modules.is_empty());
        assert!(config.go.is_some());
    }
}
