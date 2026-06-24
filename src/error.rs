//! Library error types (`thiserror`) and the binary's exit-code mapping (contracts/cli.md).

use thiserror::Error;

/// Process exit codes (contracts/cli.md "Exit codes").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum ExitCode {
    /// Success / fully compliant.
    Success = 0,
    /// Drift or violations found (non-compliant).
    Violations = 1,
    /// Usage / configuration error.
    Usage = 2,
    /// Partial apply — some files changed, some failed (FR-021).
    Partial = 3,
}

impl ExitCode {
    pub fn code(self) -> i32 {
        self as i32
    }
}

/// Library-level errors. The binary wraps these with `anyhow` and maps to [`ExitCode`].
#[derive(Debug, Error)]
pub enum LicetError {
    /// Invalid configuration or usage — maps to exit 2.
    #[error("configuration error: {0}")]
    Config(String),

    /// I/O failure.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// Git/repository discovery failure.
    #[error("git error: {0}")]
    Git(String),

    /// Internal invariant violation.
    #[error("{0}")]
    Internal(String),
}

impl LicetError {
    /// The exit code this error maps to (config/usage → 2, everything else → 2 as well,
    /// since unexpected library failures are surfaced as usage-level errors to the caller).
    pub fn exit_code(&self) -> ExitCode {
        match self {
            LicetError::Config(_) => ExitCode::Usage,
            _ => ExitCode::Usage,
        }
    }
}

/// Convenience result alias for the library.
pub type Result<T> = std::result::Result<T, LicetError>;
