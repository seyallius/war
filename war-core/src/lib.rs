//! war-core - Shared domain logic, configuration, and error types for the war CLI toolkit.
//!
//! This crate is the gravity center of the war workspace. All language-specific
//! crates (war-go, war-rust) depend on it for config management, error handling,
//! and cross-platform utilities.

#![warn(missing_docs)]

pub mod config;
pub mod error;
pub mod shell;
pub mod types;

// Re-export key types for ergonomic downstream use
pub use config::WarConfig;
pub use error::WarError;
pub use types::{ModuleInfo, SyncResult};
