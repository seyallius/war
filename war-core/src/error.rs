//! error - Unified error type for all war operations.
//!
//! Every public function in the war workspace returns `Result<_, WarError>`.
//! The CLI layer matches on these variants to produce human-friendly messages.

use std::{error, io, path::PathBuf};

/// Unified error type for all war operations.
#[derive(thiserror::Error, Debug)]
pub enum WarError {
    /// A Go command failed with non-zero exit code.
    #[error("Go command '{command}' failed (exit {exit_code}): {stderr}")]
    GoCommandFailed {
        /// The command that was executed.
        command: String,
        /// Standard error output from the command.
        stderr: String,
        /// Exit code from the command.
        exit_code: i32,
    },

    /// Failed to parse vendor/modules.txt or similar metadata.
    #[error("Failed to parse vendor file at {path}: {reason}")]
    VendorParseError {
        /// Path to the vendor file that couldn't be parsed.
        path: PathBuf,
        /// Reason for the parse failure.
        reason: String,
    },

    /// Error writing to the Go module cache during reconstruction.
    #[error("Failed to write cache for module {module}: {source}")]
    CacheWriteError {
        /// Module that failed to write.
        module: String,
        /// Source I/O error.
        #[source]
        source: io::Error,
    },

    /// Configuration file (war.lock) read/write/parsing error.
    #[error("Configuration error: {source}")]
    ConfigError {
        /// Source error that caused the configuration failure.
        #[source]
        source: Box<dyn error::Error + Send + Sync>,
    },

    /// Failed to detect or interact with the user's shell.
    #[error("Failed to detect or configure shell environment")]
    ShellDetectionError,

    /// Error creating a .zip archive during cache reconstruction.
    #[error("Failed to create zip archive for {module}: {source}")]
    ZipCreationError {
        /// Module being archived.
        module: String,
        /// Source zip error.
        #[source]
        source: zip::result::ZipError,
    },

    /// A zip archive is corrupt, truncated, or otherwise unreadable.
    ///
    /// Distinct from `ZipCreationError` (which covers *writing*) — this
    /// variant is emitted when *reading* an existing archive fails at the
    /// structural level (bad magic, truncated central directory, CRC mismatch).
    ///
    /// # Retry hint
    ///
    /// The `hint` field carries a user-facing remediation message
    /// (e.g. "Re-download or re-create the archive with `war go pack`").
    #[error("Corrupted zip archive at {path}: {reason}")]
    CorruptedArchive {
        /// Filesystem path to the corrupt archive.
        path: PathBuf,
        /// Human-readable description of what went wrong.
        reason: String,
        /// Actionable suggestion shown to the user in the CLI.
        hint: String,
    },

    /// User explicitly aborted an interactive operation.
    #[error("Operation aborted by user")]
    UserAborted,

    /// Module sync failed with partial artifacts created.
    #[error("Failed to sync {module}: {reason}")]
    ModuleSyncError {
        /// Module that failed to sync.
        module: String,
        /// Reason for the sync failure.
        reason: String,
        /// Paths to artifacts created before the failure.
        partial_artifacts: Vec<PathBuf>,
        /// Whether the operation can be retried.
        recoverable: bool,
    },

    /// I/O operation failed during file or directory access.
    #[error("I/O error: {0}")]
    IOError(#[from] io::Error),

    /// Failed to parse a file or data structure (go.mod, TOML config, etc.).
    #[error("parse error: {0}")]
    ParseError(String),

    /// Invalid input provided by the caller or user.
    #[error("invalid input: {0}")]
    InvalidInput(String),
}
impl WarError {
    /// Construct a `CorruptedArchive` error with a standard retry hint.
    ///
    /// Convenience constructor so callers don't have to repeat the boilerplate
    /// hint string at every zip-read site.
    pub fn corrupted_archive(path: impl Into<PathBuf>, reason: impl Into<String>) -> Self {
        Self::CorruptedArchive {
            path: path.into(),
            reason: reason.into(),
            hint: "Re-create the archive with `war go pack` or re-download it from your source."
                .to_string(),
        }
    }

    /// Returns `true` if this error is safe to retry after user intervention.
    ///
    /// Used by the CLI to decide whether to show a "try again" prompt.
    pub fn is_recoverable(&self) -> bool {
        match self {
            WarError::CorruptedArchive { .. } => true,
            WarError::ModuleSyncError { recoverable, .. } => *recoverable,
            WarError::InvalidInput(_) => true,
            _ => false,
        }
    }
}
