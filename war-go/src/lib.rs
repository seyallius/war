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
pub mod pack;
pub mod stage;
pub mod sync;
pub mod unpack;
#[deprecated(note = "Pivoted to airgap pack/unpack logic. See `README.md` for more details.")]
pub mod vendor;
pub mod verify;

// Re-export key public APIs for ergonomic use by war-cli and war-tui
pub use get::fetch_module;
pub use get::fetch_module_with_go_path;
pub use init::init_project;
pub use offline::{
    default_cache_root, generate_offline_exports, generate_online_exports, go_offline,
};
pub use online::go_online;
pub use pack::pack_modules;
pub use stage::{
    add_staged, clear_staged, get_staged_filter, get_staged_modules, list_staged, remove_staged,
};
pub use sync::{resolve_gomodcache, sync_cache, CollisionWarning, SyncResult, SyncStats};
pub use unpack::{unpack_modules, unpack_modules_with_opts, UnpackOpts, UnpackStats};
pub use vendor::{parse_modules_txt, parse_vendor_manifest};
pub use verify::verify_offline;
