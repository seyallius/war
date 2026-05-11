//! dispatch.rs - Central command dispatcher for the `war` CLI.
//!
//! Routes parsed top-level CLI commands to the appropriate language-specific
//! handlers. Each command variant is forwarded to its corresponding executor
//! in `war-go`, `war-core`, or future language modules. The dispatcher returns
//! an integer exit code where `0` indicates success and `1` signals an error.
//!
//! This module acts as the control flow hub between the CLI layer and the
//! underlying offline-development logic.

use crate::{cli::Commands, commands::GoCommands};
use std::path::PathBuf;

// --------------------------------------------- Public (Crate) API ---------------------------------------------

/// Dispatch a parsed `Commands` variant to the appropriate handler.
/// Returns an exit code: 0 on success, 1 on error.
pub(crate) async fn dispatch(command: &Commands) -> i32 {
    match command {
        Commands::Cargo { .. } => {
            tracing::error!("Rust support is not yet implemented. Coming soon though…");
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
            let project_root = std::env::current_dir().unwrap_or_else(|e| {
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

        GoCommands::Pack { cache, output } => {
            let cache_path = match resolve_cache_path(cache) {
                Ok(p) => p,
                Err(e) => {
                    tracing::error!("✘ {}", e);
                    return 1;
                }
            };
            let output_path = PathBuf::from(output);

            tracing::info!(
                "Packing cache from {} → {}",
                cache_path.display(),
                output_path.display()
            );

            match war_go::pack_modules(&cache_path, &output_path, None).await {
                Ok(()) => {
                    tracing::info!("✔ Archive written to {}", output_path.display());
                    0
                }
                Err(e) => {
                    tracing::error!("✘ Pack failed: {}", e);
                    1
                }
            }
        }

        GoCommands::Unpack {
            archive,
            cache,
            dry_run,
        } => {
            let archive_path = PathBuf::from(archive);
            let cache_path = match resolve_cache_path(cache) {
                Ok(p) => p,
                Err(e) => {
                    tracing::error!("✘ {}", e);
                    return 1;
                }
            };

            let opts = war_go::UnpackOpts { dry_run: *dry_run };

            if *dry_run {
                tracing::info!(
                    "[dry-run] Would unpack {} → {}",
                    archive_path.display(),
                    cache_path.display()
                );
            } else {
                tracing::info!(
                    "Unpacking {} → {}",
                    archive_path.display(),
                    cache_path.display()
                );
            }

            match war_go::unpack_modules_with_opts(&archive_path, &cache_path, &opts) {
                Ok(stats) => {
                    if *dry_run {
                        tracing::info!(
                            "[dry-run] ✔ {} files would be extracted, {} skipped, {} failed",
                            stats.extracted,
                            stats.skipped,
                            stats.failed
                        );
                    } else {
                        tracing::info!(
                            "✔ Unpack complete: {} extracted, {} skipped, {} failed",
                            stats.extracted,
                            stats.skipped,
                            stats.failed
                        );
                    }
                    if stats.failed > 0 {
                        1
                    } else {
                        0
                    }
                }
                Err(e) => {
                    tracing::error!("✘ Unpack failed: {}", e);
                    1
                }
            }
        }

        GoCommands::Offline { vendor: _, global } => {
            tracing::info!("Enabling offline mode (global: {})", global);
            let exports = war_go::generate_offline_exports();
            // Print the export statements to stdout so the user can
            // `eval $(war go offline)` them into their shell.
            println!("{}", exports);
            0
        }

        GoCommands::Online { global } => {
            tracing::info!("Restoring online mode (global: {})", global);
            let exports = war_go::generate_online_exports();
            println!("{}", exports);
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
            tracing::info!("Verifying offline configuration…");
            match war_go::verify_offline().await {
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

/// Resolve the cache path from an optional CLI argument, falling back to
/// `~/.war/cache/go` if not provided.
fn resolve_cache_path(opt: &Option<String>) -> Result<PathBuf, String> {
    match opt {
        Some(p) => Ok(PathBuf::from(p)),
        None => war_go::default_cache_root().map_err(|e| e.to_string()),
    }
}
