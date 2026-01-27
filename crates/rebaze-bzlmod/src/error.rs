//! Error types for rebaze-bzlmod.

use thiserror::Error;

/// Result type alias using our Error type.
pub type Result<T> = std::result::Result<T, Error>;

/// Errors that can occur during module resolution.
#[derive(Debug, Error)]
pub enum Error {
    /// Failed to parse MODULE.bazel file.
    #[error("parse error: {0}")]
    Parse(String),

    /// Registry communication error.
    #[error("registry error: {0}")]
    Registry(#[from] RegistryError),

    /// Resolution failed.
    #[error("resolution error: {0}")]
    Resolution(String),

    /// Invalid module name.
    #[error("invalid module name: {0}")]
    InvalidModuleName(String),

    /// Invalid version.
    #[error("invalid version: {0}")]
    InvalidVersion(String),

    /// Module not found in any registry.
    #[error("module not found: {0}")]
    ModuleNotFound(String),

    /// Version not found for module.
    #[error("version {version} not found for module {module}")]
    VersionNotFound {
        /// Module name.
        module: String,
        /// Requested version.
        version: String,
    },

    /// Yanked version encountered.
    #[error("yanked version: {module}@{version} - {reason}")]
    YankedVersion {
        /// Module name.
        module: String,
        /// Yanked version.
        version: String,
        /// Reason for yanking.
        reason: String,
    },

    /// Maximum resolution depth exceeded.
    #[error("maximum resolution depth ({0}) exceeded")]
    MaxDepthExceeded(usize),

    /// Circular dependency detected.
    #[error("circular dependency: {0}")]
    CircularDependency(String),

    /// IO error.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// JSON parsing error.
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
}

/// Registry-specific errors.
#[derive(Debug, Error)]
pub enum RegistryError {
    /// HTTP request failed.
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),

    /// HTTP status code error.
    #[error("http status {status}: {url}")]
    HttpStatus {
        /// HTTP status code.
        status: u16,
        /// Request URL.
        url: String,
    },

    /// Invalid registry URL.
    #[error("invalid registry url: {0}")]
    InvalidUrl(String),

    /// Registry not reachable.
    #[error("registry not reachable: {0}")]
    NotReachable(String),

    /// Invalid metadata response.
    #[error("invalid metadata: {0}")]
    InvalidMetadata(String),
}
