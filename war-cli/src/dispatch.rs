//! dispatch.rs - Central command dispatcher for the `war` CLI.
//!
//! Routes parsed top-level CLI commands to the appropriate language-specific
//! handlers. Each command variant is forwarded to its corresponding executor
//! in `war-go`, `war-core`, or future language modules. The dispatcher returns
//! an integer exit code where `0` indicates success and `1` signals an error.
//!
//! This module acts as the control flow hub between the CLI layer and the
//! underlying offline-development logic.

use crate::{cli::Commands, commands::GoCommands, commands::StageCommands};
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
                    tracing::info!("✔ Module '{}' fetched and staged.", module);
                    0
                }
                Err(e) => {
                    tracing::error!("✘ Failed to fetch module '{}': {}", module, e);
                    1
                }
            }
        }

        GoCommands::Pack {
            cache,
            output,
            staged,
        } => {
            let cache_path = match resolve_cache_path(cache) {
                Ok(p) => p,
                Err(e) => {
                    tracing::error!("✘ {}", e);
                    return 1;
                }
            };
            let output_path = PathBuf::from(output);

            // Resolve staged filter if --staged was passed.
            let staged_filter = if *staged {
                match war_go::get_staged_filter() {
                    Ok(filter) => {
                        if filter.is_empty() {
                            tracing::warn!(
                                "⚠ --staged flag set but staged list is empty. \
                                 Use `war go stage add <module> <version>` first."
                            );
                        }
                        Some(filter)
                    }
                    Err(e) => {
                        tracing::error!("✘ Failed to load staged modules: {}", e);
                        return 1;
                    }
                }
            } else {
                None
            };

            tracing::info!(
                "Packing cache from {} → {}{}",
                cache_path.display(),
                output_path.display(),
                if *staged { " (staged only)" } else { "" }
            );

            match war_go::pack_modules(&cache_path, &output_path, staged_filter).await {
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
            staged,
        } => {
            let archive_path = PathBuf::from(archive);
            let cache_path = match resolve_cache_path(cache) {
                Ok(p) => p,
                Err(e) => {
                    tracing::error!("✘ {}", e);
                    return 1;
                }
            };

            // Resolve staged filter if --staged was passed.
            let staged_filter = if *staged {
                match war_go::get_staged_filter() {
                    Ok(filter) => {
                        if filter.is_empty() {
                            tracing::warn!(
                                "⚠ --staged flag set but staged list is empty. \
                                 Use `war go stage add <module> <version>` first."
                            );
                        }
                        Some(filter)
                    }
                    Err(e) => {
                        tracing::error!("✘ Failed to load staged modules: {}", e);
                        return 1;
                    }
                }
            } else {
                None
            };

            let opts = war_go::UnpackOpts {
                dry_run: *dry_run,
                staged_filter,
            };

            if *dry_run {
                tracing::info!(
                    "[dry-run] Would unpack {} → {}",
                    archive_path.display(),
                    cache_path.display()
                );
            } else {
                tracing::info!(
                    "Unpacking {} → {}{}",
                    archive_path.display(),
                    cache_path.display(),
                    if *staged { " (staged only)" } else { "" }
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

        GoCommands::Stage { subcommand } => dispatch_stage(subcommand),

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

        GoCommands::Sync { cache, dest } => {
            // Resolve the source: CLI flag → default war cache.
            let src = match resolve_cache_path(cache) {
                Ok(p) => p,
                Err(e) => {
                    tracing::error!("✘ {}", e);
                    return 1;
                }
            };

            // Resolve the destination: CLI flag → $GOMODCACHE → ~/go/pkg/mod.
            let dst = if let Some(d) = dest {
                PathBuf::from(d)
            } else {
                match war_go::resolve_gomodcache() {
                    Ok(p) => p,
                    Err(e) => {
                        tracing::error!("✘ Failed to resolve native Go module cache: {}", e);
                        return 1;
                    }
                }
            };

            tracing::info!(
                "(◕‿◕✿) Syncing war cache → native Go module cache…\n  src: {}\n  dst: {}",
                src.display(),
                dst.display()
            );

            match war_go::sync_cache(&src, &dst) {
                Ok(result) => {
                    let stats = &result.stats;

                    tracing::info!(
                        "✔ Sync complete: {} copied, {} skipped, {} collisions, {} failed",
                        stats.copied,
                        stats.skipped,
                        stats.collisions,
                        stats.failed
                    );

                    // Surface any collision warnings so the user can investigate.
                    if !result.warnings.is_empty() {
                        tracing::warn!(
                            "⚠ {} collision(s) detected — existing files were NOT overwritten:",
                            result.warnings.len()
                        );
                        for w in &result.warnings {
                            tracing::warn!(
                                "  • {}\n      src  sha256: {}\n      dst  sha256: {}",
                                w.destination.display(),
                                w.source_hash,
                                w.destination_hash
                            );
                        }
                        tracing::warn!("  Resolve collisions manually, then re-run `war go sync`.");
                    }

                    // Exit 1 only on hard failures, not on collisions or skips.
                    // Collisions are warnings, not errors — the user is informed.
                    if stats.failed > 0 {
                        1
                    } else {
                        0
                    }
                }
                Err(e) => {
                    tracing::error!("✘ Sync failed: {}", e);
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

/// Dispatch a `StageCommands` variant.
fn dispatch_stage(subcommand: &StageCommands) -> i32 {
    match subcommand {
        StageCommands::List => match war_go::list_staged() {
            Ok(entries) => {
                if entries.is_empty() {
                    println!("No staged modules. Use `war go stage add <module> <version>` or `war go get <module>`.");
                } else {
                    println!("Staged modules ({}):", entries.len());
                    for entry in &entries {
                        println!("  {}", entry);
                    }
                }
                0
            }
            Err(e) => {
                tracing::error!("✘ Failed to list staged modules: {}", e);
                1
            }
        },

        StageCommands::Add { module, version } => {
            tracing::info!("Staging {}@{}", module, version);
            match war_go::add_staged(module, version) {
                Ok(true) => {
                    tracing::info!("✔ Staged {}@{}", module, version);
                    0
                }
                Ok(false) => {
                    tracing::info!(
                        "{}@{} already staged — no duplicate added (◕‿◕)",
                        module,
                        version
                    );
                    0
                }
                Err(e) => {
                    tracing::error!("✘ Failed to stage {}@{}: {}", module, version, e);
                    1
                }
            }
        }

        StageCommands::Remove { module, version } => {
            tracing::info!("Unstaging {}@{}", module, version);
            match war_go::remove_staged(module, version) {
                Ok(true) => {
                    tracing::info!("✔ Unstaged {}@{}", module, version);
                    0
                }
                Ok(false) => {
                    tracing::info!("{}@{} was not in the staged list", module, version);
                    0
                }
                Err(e) => {
                    tracing::error!("✘ Failed to unstage {}@{}: {}", module, version, e);
                    1
                }
            }
        }

        StageCommands::Clear => {
            tracing::info!("Clearing all staged modules");
            match war_go::clear_staged() {
                Ok(()) => {
                    tracing::info!("✔ Staged modules cleared");
                    0
                }
                Err(e) => {
                    tracing::error!("✘ Failed to clear staged modules: {}", e);
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
