//! error - Centralized error types for the war toolkit using thiserror.
//!
//! All language-specific crates map their internal errors into this enum.
//! The CLI layer matches on these variants to produce human-friendly messages.

use std::{error, io, path::PathBuf};

/// Unified error type for all war operations.
#[derive(thiserror::Error, Debug)]
pub enum WarError {
    /// A Go command failed with non-zero exit code.
    #[error("Go command '{command}' failed (exit {exit_code}): {stderr}")]
    GoCommandFailed {
        /// The command that was executed
        command: String,
        /// Standard error output from the command
        stderr: String,
        /// Exit code from the command
        exit_code: i32,
    },

    /// Failed to parse vendor/modules.txt or similar metadata.
    #[error("Failed to parse vendor file at {path}: {reason}")]
    VendorParseError {
        /// Path to the vendor file that couldn't be parsed
        path: PathBuf,
        /// Reason for the parse failure
        reason: String,
    },

    /// Error writing to the Go module cache during reconstruction.
    #[error("Failed to write cache for module {module}: {source}")]
    CacheWriteError {
        /// Module that failed to write
        module: String,
        /// Source I/O error
        #[source]
        source: io::Error,
    },

    /// Configuration file (war.lock) read/write/parsing error.
    #[error("Configuration error: {source}")]
    ConfigError {
        /// Source error that caused the configuration failure
        #[source]
        source: Box<dyn error::Error + Send + Sync>,
    },

    /// Failed to detect or interact with the user's shell.
    #[error("Failed to detect or configure shell environment")]
    ShellDetectionError,

    /// Error creating a .zip archive during cache reconstruction.
    #[error("Failed to create zip archive for {module}: {source}")]
    ZipCreationError {
        /// Module being archived
        module: String,
        /// Source zip error
        #[source]
        source: zip::result::ZipError,
    },

    /// User explicitly aborted an interactive operation.
    #[error("Operation aborted by user")]
    UserAborted,

    /// Module sync failed with partial artifacts created.
    #[error("Failed to sync {module}: {reason}")]
    ModuleSyncError {
        /// Module that failed to sync
        module: String,
        /// Reason for the sync failure
        reason: String,
        /// Paths to artifacts created before the failure
        partial_artifacts: Vec<PathBuf>,
        /// Whether the operation can be retried
        recoverable: bool,
    },
}
