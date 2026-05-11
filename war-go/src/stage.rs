//! stage.rs - Manage the staged-module cart for `war go pack --staged`.
//!
//! Provides the CLI-facing operations for `war go stage list`,
//! `war go stage add`, `war go stage remove`, and `war go stage clear`.
//! All mutations go through `war_core::config` which handles atomic TOML
//! persistence to `~/.war/war.lock`.
//!
//! The staged list is also populated automatically by `war go get`, so
//! the typical workflow is:
//!
//! ```text
//! war go get github.com/gin-gonic/gin@v1.9.1   # auto-stages
//! war go stage list                              # shows gin@v1.9.1
//! war go pack --staged staged-cache.zip          # packs only staged
//! ```

use war_core::{config, StagedModule, WarError};

// ----------------------- Public Functions -----------------------

/// List all staged modules, returning them as formatted strings suitable
/// for printing to the terminal.
///
/// Each line has the form `module_path@version`.  The list is sorted
/// alphabetically by module path then version (matching the TOML order).
pub fn list_staged() -> Result<Vec<String>, WarError> {
    let modules = config::stage_list()?;
    let lines: Vec<String> = modules
        .iter()
        .map(|sm| format!("{}@{}", sm.module, sm.version))
        .collect();
    tracing::info!("Staged modules: {} entries", lines.len());
    Ok(lines)
}

/// Add a module to the staged list.
///
/// Delegates to `war_core::config::stage_add` which handles deduplication
/// and atomic persistence.  Returns `Ok(true)` if the module was newly
/// added, `Ok(false)` if it was already present.
pub fn add_staged(module: &str, version: &str) -> Result<bool, WarError> {
    config::stage_add(module, version)
}

/// Remove a module from the staged list.
///
/// Returns `Ok(true)` if a module was removed, `Ok(false)` if the
/// module was not found.
pub fn remove_staged(module: &str, version: &str) -> Result<bool, WarError> {
    config::stage_remove(module, version)
}

/// Clear all staged modules.
pub fn clear_staged() -> Result<(), WarError> {
    config::stage_clear()
}

/// Return the staged list as `(module, version)` filter pairs, ready
/// for use with `pack_modules` or `unpack_modules_with_opts`.
pub fn get_staged_filter() -> Result<Vec<(String, String)>, WarError> {
    config::staged_filter_pairs()
}

/// Return the raw `Vec<StagedModule>` for callers that need the
/// structured type.
pub fn get_staged_modules() -> Result<Vec<StagedModule>, WarError> {
    config::stage_list()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_list_staged_format() {
        let modules = vec![
            StagedModule {
                module: "github.com/gin-gonic/gin".into(),
                version: "v1.9.1".into(),
            },
            StagedModule {
                module: "golang.org/x/text".into(),
                version: "v0.3.7".into(),
            },
        ];

        let lines: Vec<String> = modules
            .iter()
            .map(|sm| format!("{}@{}", sm.module, sm.version))
            .collect();

        assert_eq!(lines[0], "github.com/gin-gonic/gin@v1.9.1");
        assert_eq!(lines[1], "golang.org/x/text@v0.3.7");
    }
}
