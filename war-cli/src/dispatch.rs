//! dispatch - Central command dispatcher for the `war` CLI.
//!
//! Routes parsed top‑level CLI commands to the appropriate language‑specific
//! handlers. Each command variant is forwarded to its corresponding executor
//! in `war-go`, `war-core`, or future language modules. The dispatcher returns
//! an integer exit code where `0` indicates success and `1` signals an error.
//!
//! This module acts as the control flow hub between the CLI layer and the
//! underlying offline‑development logic.

use crate::{cli::Commands, commands::GoCommands};
use std::{env, path::PathBuf};

// --------------------------------------------- Public (Crate) API ---------------------------------------------

/// Dispatch a parsed `Commands` variant to the appropriate handler.
/// Returns an exit code: 0 on success, 1 on error.
pub(crate) async fn dispatch(command: &Commands) -> i32 {
    match command {
        Commands::Rust { .. } => {
            tracing::error!("Rust support is not yet implemented. Coming soon though...");
            1
        }
        Commands::Go { subcommand } => dispatch_go(subcommand).await,
    }
}

// --------------------------------------------- Internal Helpers ---------------------------------------------

/// Dispatch a parsed `GoCommands` variant to the corresponding `war_go` function.
/// Returns an exit code: 0 on success, 1 on error.
async fn dispatch_go(subcommand: &GoCommands) -> i32 {
    match subcommand {
        GoCommands::Init { name } => {
            tracing::info!("Initializing Go project: {}", name);
            match war_go::init_project(name).await {
                Ok(path) => {
                    tracing::info!("✔ Project '{}' initialized at: {}", name, path.display());
                    0
                }
                Err(e) => {
                    tracing::error!("✘ Failed to initialize project '{}': {}", name, e);
                    1
                }
            }
        }

        GoCommands::Get { module } => {
            let project_root = env::current_dir().unwrap_or_else(|e| {
                tracing::warn!(
                    "Could not determine current directory: {}. Falling back to '.'",
                    e
                );
                PathBuf::from(".")
            });

            tracing::info!(
                "Fetching module '{}' in project root: {}",
                module,
                project_root.display()
            );

            match war_go::fetch_module(module, &project_root).await {
                Ok(()) => {
                    tracing::info!("✔ Module '{}' fetched successfully.", module);
                    0
                }
                Err(e) => {
                    tracing::error!("✘ Failed to fetch module '{}': {}", module, e);
                    1
                }
            }
        }

        GoCommands::Offline { vendor, global } => {
            let vendor_path = vendor.as_ref().map(PathBuf::from);
            tracing::info!(
                "Enabling offline mode (global: {}, vendor: {:?})",
                global,
                vendor_path
            );

            match war_go::go_offline(vendor_path, *global) {
                Ok(changes) => {
                    tracing::info!("✔ Offline mode enabled.");
                    if !changes.is_empty() {
                        tracing::info!("  Environment variables modified:");
                        for (key, _value) in &changes {
                            tracing::info!("    • {}", key);
                        }
                    }
                    0
                }
                Err(e) => {
                    tracing::error!("✘ Failed to enable offline mode: {}", e);
                    1
                }
            }
        }

        GoCommands::Online { global } => {
            tracing::info!("Restoring online mode (global: {})", global);
            match war_go::go_online(*global) {
                Ok(()) => {
                    tracing::info!("✔ Online mode restored.");
                    0
                }
                Err(e) => {
                    tracing::error!("✘ Failed to restore online mode: {}", e);
                    1
                }
            }
        }

        GoCommands::Verify => {
            tracing::info!("Verifying offline configuration...");
            match war_go::verify_offline() {
                Ok(()) => {
                    tracing::info!("✔ Offline mode verified — no network fallback detected.");
                    0
                }
                Err(e) => {
                    tracing::error!("✘ Offline verification failed: {}", e);
                    1
                }
            }
        }
    }
}
