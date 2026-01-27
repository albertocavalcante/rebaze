//! Configuration system for rebaze migrations.
//!
//! This module provides an extensible configuration system designed to support
//! migrations between multiple build systems. Currently supports Bazel as a target,
//! with architecture ready for future targets (Gradle, Buck2, etc.).
//!
//! # TOML Structure
//!
//! The configuration file (`rebaze.toml`) supports both flat and nested structures:
//!
//! ```toml
//! # Flat structure (current, implies Bazel target)
//! [versions]
//! rules_cc = "0.1.1"
//!
//! # Nested structure (future-proof, explicit target)
//! [bazel.versions]
//! rules_cc = "0.1.1"
//!
//! # Future targets
//! [gradle.versions]
//! rules_jvm_external = "6.0"
//! ```

use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

// Re-export target-specific configs for convenience
pub use rebaze_bazel::config::MigrationConfig as BazelConfig;

/// Top-level configuration for rebaze.
///
/// This struct holds configuration for all supported migration targets.
/// Each target's config is optional - if not specified, defaults are used.
///
/// # Extensibility
///
/// To add a new target:
/// 1. Create a `FooConfig` in the target crate (e.g., `rebaze-gradle-gen`)
/// 2. Add `pub foo: Option<FooConfig>` field here
/// 3. Update `load_from_file` to parse `[foo]` section
/// 4. Update generators to accept the config
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Bazel target configuration.
    /// Controls Bazel dependency versions, mappings, filters, and build options.
    #[serde(default)]
    pub bazel: Option<BazelConfig>,

    // Future targets:
    // pub gradle: Option<GradleConfig>,
    // pub buck2: Option<Buck2Config>,
}

impl Config {
    /// Load configuration from a TOML file.
    ///
    /// Supports two TOML formats:
    /// 1. Nested: `[bazel.versions]` - explicit target sections
    /// 2. Flat: `[versions]` - legacy format, interpreted as Bazel config
    pub fn load_from_file(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read config file: {}", path.display()))?;

        Self::parse_toml(&content)
            .with_context(|| format!("Failed to parse config file: {}", path.display()))
    }

    /// Parse TOML content into Config.
    fn parse_toml(content: &str) -> Result<Self> {
        // First, try parsing as nested format with explicit [bazel] section
        let value: toml::Value = toml::from_str(content)?;

        if let Some(table) = value.as_table() {
            // Check if there's an explicit [bazel] section
            if table.contains_key("bazel") {
                // Nested format: parse directly
                let config: Config = toml::from_str(content)?;
                return Ok(config);
            }

            // Flat format: treat entire file as Bazel config
            let bazel_config: BazelConfig = toml::from_str(content)?;
            return Ok(Config {
                bazel: Some(bazel_config),
            });
        }

        // Empty or invalid - return defaults
        Ok(Config::default())
    }

    /// Load configuration from a project directory.
    ///
    /// Looks for `rebaze.toml` in the given directory.
    /// Returns default config if file doesn't exist.
    pub fn load_from_project(project_dir: &Path) -> Self {
        let config_path = project_dir.join("rebaze.toml");
        Self::load_from_path_or_default(Some(&config_path), project_dir)
    }

    /// Load configuration from an explicit path, or discover from project directory.
    ///
    /// - If `config_path` is `Some`, loads from that path (error if not found)
    /// - If `config_path` is `None`, tries to discover `rebaze.toml` in `project_dir`
    /// - Falls back to defaults if no config found during discovery
    pub fn load_from_path_or_discover(
        config_path: Option<&Path>,
        project_dir: &Path,
    ) -> Result<Self> {
        match config_path {
            Some(path) => {
                // Explicit path - must exist
                let config = Self::load_from_file(path)?;
                tracing::info!("Loaded configuration from {}", path.display());
                Ok(config)
            }
            None => {
                // Discovery mode - optional
                Ok(Self::load_from_project(project_dir))
            }
        }
    }

    /// Load from path or return defaults (non-failing version).
    fn load_from_path_or_default(config_path: Option<&Path>, project_dir: &Path) -> Self {
        let path = config_path.map_or_else(|| project_dir.join("rebaze.toml"), Path::to_path_buf);

        if !path.exists() {
            tracing::debug!("No config file found at {}, using defaults", path.display());
            return Self::default();
        }

        match Self::load_from_file(&path) {
            Ok(config) => {
                tracing::info!("Loaded configuration from {}", path.display());
                config
            }
            Err(e) => {
                tracing::warn!(
                    "Failed to load {}: {}. Using defaults.",
                    path.display(),
                    e
                );
                Self::default()
            }
        }
    }

    /// Get Bazel configuration, using defaults if not specified.
    #[must_use]
    pub fn bazel_config(&self) -> BazelConfig {
        self.bazel.clone().unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_flat_toml_format() {
        let content = r#"
[versions]
rules_cc = "0.2.0"
googletest = "1.16.0"

[strategy]
default = "source"
"#;
        let config = Config::parse_toml(content).unwrap();
        let bazel = config.bazel_config();
        assert_eq!(bazel.versions.rules_cc, "0.2.0");
        assert_eq!(bazel.versions.googletest, "1.16.0");
        assert_eq!(bazel.strategy.default, "source");
    }

    #[test]
    fn test_nested_toml_format() {
        let content = r#"
[bazel.versions]
rules_cc = "0.3.0"

[bazel.strategy]
default = "source"
"#;
        let config = Config::parse_toml(content).unwrap();
        let bazel = config.bazel_config();
        assert_eq!(bazel.versions.rules_cc, "0.3.0");
        assert_eq!(bazel.strategy.default, "source");
    }

    #[test]
    fn test_empty_config_uses_defaults() {
        let config = Config::default();
        let bazel = config.bazel_config();
        assert_eq!(bazel.versions.rules_cc, "0.1.1"); // Default value
    }

    #[test]
    fn test_partial_config_merges_with_defaults() {
        let content = r#"
[versions]
rules_cc = "0.5.0"
"#;
        let config = Config::parse_toml(content).unwrap();
        let bazel = config.bazel_config();
        assert_eq!(bazel.versions.rules_cc, "0.5.0");
        // Other fields should have defaults
        assert_eq!(bazel.versions.googletest, "1.15.2");
    }
}
