//! war-go - Go-specific domain logic for the war offline development toolkit.

#![warn(missing_docs)]

pub mod cache;
pub mod get;
pub mod init;
pub mod offline;
pub mod online;
pub mod pack;
pub mod unpack;
#[deprecated(note = "Pivoted to airgap pack/unpack logic. See `README.md` for more details.")]
pub mod vendor;
pub mod verify;

// Re-export key public APIs for ergonomic use by war-cli and war-tui
pub use get::fetch_module;
pub use get::fetch_module_with_go_path;
pub use init::init_project;
pub use offline::{generate_offline_exports, go_offline};
pub use online::go_online;
pub use pack::pack_modules;
pub use unpack::{unpack_modules, UnpackStats};
pub use vendor::{parse_modules_txt, parse_vendor_manifest};
pub use verify::verify_offline;
