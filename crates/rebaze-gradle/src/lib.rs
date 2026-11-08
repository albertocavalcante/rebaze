//! Source extraction for a conservative literal Gradle Java subset.

use anyhow::{Result, bail};
use std::path::Path;

mod input;
mod java;
mod model;
mod parser;
mod syntax;
pub use model::*;
pub use rebaze_input::ReadLimits;

/// Source/IO diagnostics retain their context behind a library error type.
#[derive(Debug, thiserror::Error)]
#[error("invalid Gradle Java input: {0:#}")]
pub struct GradleError(#[source] anyhow::Error);

pub fn parse_build_source(source: &str) -> std::result::Result<GradleModule, GradleError> {
    parser::parse_build_source(source).map_err(GradleError)
}

/// Normalize a literal Gradle project path without admitting filesystem traversal.
pub fn project_path(value: &str) -> std::result::Result<String, GradleError> {
    normalize_project_path(value).map_err(GradleError)
}

fn normalize_project_path(value: &str) -> Result<String> {
    let bare = value.strip_prefix(':').unwrap_or(value);
    if bare.len() > 4095 {
        bail!("Gradle project path exceeds 4096-byte normalized path budget");
    }
    if value.is_empty() || value == ":" {
        return Ok(String::new());
    }
    let value = value.strip_prefix(':').unwrap_or(value);
    if value.split(':').any(|part| {
        part.is_empty()
            || !part
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-'))
    }) {
        bail!("unsupported Gradle project path {value:?}");
    }
    Ok(format!(":{value}"))
}

pub fn parse(path: &Path) -> std::result::Result<GradleProject, GradleError> {
    parse_with_limits(path, ReadLimits::default())
}

/// Capture the literal Java subset using one root and shared IO budget.
pub fn parse_with_limits(
    path: &Path,
    limits: ReadLimits,
) -> std::result::Result<GradleProject, GradleError> {
    input::read_project(path, limits).map_err(GradleError)
}
