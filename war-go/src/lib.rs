//! war-go - Go-specific domain logic for the war offline development toolkit.
//!
//! This crate encapsulates all knowledge about Go module management, vendor parsing,
//! cache reconstruction, and environment toggling. It depends only on war-core for
//! shared types, config, and error handling — keeping domain logic isolated and testable.

#![warn(missing_docs)]

pub mod cache;
pub mod get;
pub mod init;
pub mod offline;
pub mod online;
pub mod vendor;
pub mod verify;

// Re-export key public APIs for ergonomic use by war-cli and war-tui
pub use get::fetch_module;
pub use init::init_project;
pub use offline::go_offline;
pub use online::go_online;
pub use verify::verify_offline;
