//! Error type used across the UPKG tool.
//!
//! Names used here are generic implementation-level names (not format names),
//! so they are outside the scope of the spec's naming rule.

use std::fmt;

/// Result alias used throughout the crate.
pub type Result<T> = std::result::Result<T, UpkgError>;

/// The single error type of the tool.
#[derive(Debug)]
pub enum UpkgError {
    /// Underlying I/O failure.
    Io(std::io::Error),
    /// The package format is malformed or unsupported.
    Format(String),
    /// A hash/signature check failed.
    Verify(String),
    /// Install-time rejection (OS mismatch, conflict, invalid signature, ...).
    Reject(String),
    /// User configuration is invalid (create config, install config).
    Config(String),
    /// HTTP/network failure.
    Http(String),
    /// User aborted an interactive prompt.
    Aborted,
}

impl UpkgError {
    /// Wrap an IO error with extra context.
    pub fn io_context(e: std::io::Error, context: &str) -> UpkgError {
        UpkgError::Io(std::io::Error::new(e.kind(), format!("{context}: {e}")))
    }
}

impl fmt::Display for UpkgError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            UpkgError::Io(e) => write!(f, "io error: {e}"),
            UpkgError::Format(s) => write!(f, "format error: {s}"),
            UpkgError::Verify(s) => write!(f, "verification failed: {s}"),
            UpkgError::Reject(s) => write!(f, "rejected: {s}"),
            UpkgError::Config(s) => write!(f, "config error: {s}"),
            UpkgError::Http(s) => write!(f, "http error: {s}"),
            UpkgError::Aborted => write!(f, "aborted by user"),
        }
    }
}

impl std::error::Error for UpkgError {}

impl From<std::io::Error> for UpkgError {
    fn from(e: std::io::Error) -> Self {
        UpkgError::Io(e)
    }
}

impl From<String> for UpkgError {
    fn from(s: String) -> Self {
        UpkgError::Format(s)
    }
}

impl From<toml::de::Error> for UpkgError {
    fn from(e: toml::de::Error) -> Self {
        UpkgError::Config(format!("invalid TOML: {e}"))
    }
}

impl From<ureq::Error> for UpkgError {
    fn from(e: ureq::Error) -> Self {
        UpkgError::Http(format!("{e}"))
    }
}

impl From<serde_json::Error> for UpkgError {
    fn from(e: serde_json::Error) -> Self {
        UpkgError::Config(format!("invalid JSON: {e}"))
    }
}
