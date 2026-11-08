//! Load native migration configuration without silently discarding invalid input.
//!
//! A file uses either flat sections such as `[build]` or a single `[bazel]` tree
//! such as `[bazel.build]`. Mixed formats and unknown sections/fields are errors.
//! Overrides currently apply to native CMake generation; Gradle Java and bazelle
//! accept only default configuration. Generator support is checked before effects.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("failed to read configuration {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("invalid configuration: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("configuration cannot mix flat settings with a [bazel] section")]
    MixedFormats,
    #[error("invalid configuration {path}: {source}")]
    File {
        path: PathBuf,
        #[source]
        source: Box<Self>,
    },
}

type Result<T> = std::result::Result<T, ConfigError>;

// Re-export target-specific configs for convenience
pub use rebaze_bazel::config::MigrationConfig as BazelConfig;

/// Optional Bazel configuration loaded from the supported flat or nested form.
/// An absent configuration selects the same effective defaults as an empty file.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Bazel target configuration.
    /// Controls Bazel dependency versions, mappings, filters, and build options.
    #[serde(default)]
    pub bazel: Option<BazelConfig>,
}

impl Config {
    /// Load configuration from a TOML file.
    ///
    /// Supports two TOML formats:
    /// 1. Nested: `[bazel.versions]` - explicit target sections
    /// 2. Flat: `[versions]` - legacy format, interpreted as Bazel config
    pub fn load_from_file(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path).map_err(|source| ConfigError::Read {
            path: path.into(),
            source,
        })?;
        Self::parse_toml(&content).map_err(|source| ConfigError::File {
            path: path.into(),
            source: Box::new(source),
        })
    }

    /// Parse TOML content into Config.
    fn parse_toml(content: &str) -> Result<Self> {
        let table: toml::Table = toml::from_str(content)?;
        if table.contains_key("bazel") {
            if ["versions", "mappings", "filters", "build", "strategy"]
                .iter()
                .any(|field| table.contains_key(*field))
            {
                return Err(ConfigError::MixedFormats);
            }
            Ok(toml::from_str(content)?)
        } else {
            Ok(Self {
                bazel: Some(toml::from_str(content)?),
            })
        }
    }

    /// Load configuration from a project directory.
    ///
    /// Looks for `rebaze.toml` in the given directory.
    /// Returns default config if file doesn't exist.
    pub fn load_from_project(project_dir: &Path) -> Result<Self> {
        Self::load_from_path_or_discover(None, project_dir)
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
        if let Some(path) = config_path {
            // Explicit path - must exist
            let config = Self::load_from_file(path)?;
            tracing::info!("Loaded configuration from {}", path.display());
            Ok(config)
        } else {
            let path = project_dir.join("rebaze.toml");
            match std::fs::symlink_metadata(&path) {
                Ok(_) => Self::load_from_file(&path),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
                Err(error) => Err(ConfigError::Read {
                    path,
                    source: error,
                }),
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
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn unknown_or_mixed_configuration_fields_are_rejected() {
        for content in [
            "[build]\ndefault_visiblity = \"//visibility:private\"",
            "[bazel]\nunknown = true",
            "[gradle]\nunknown = true",
            "[build]\nmodule_version = \"2.0\"\n[bazel.build]\nmodule_version = \"3.0\"",
        ] {
            assert!(
                Config::parse_toml(content).is_err(),
                "silently accepted {content}"
            );
        }
    }

    #[test]
    fn project_loaders_reject_invalid_files_instead_of_defaulting() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(
            Config::load_from_project(root.path())
                .unwrap()
                .bazel_config(),
            BazelConfig::default()
        );
        assert_eq!(
            BazelConfig::load_from_project(root.path()).unwrap(),
            BazelConfig::default()
        );
        let path = root.path().join("rebaze.toml");
        std::fs::write(&path, "[build\n").unwrap();
        assert!(Config::load_from_project(root.path()).is_err());
        assert!(BazelConfig::load_from_project(root.path()).is_err());
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        assert!(Config::load_from_project(root.path()).is_err());
        assert!(BazelConfig::load_from_project(root.path()).is_err());
    }

    #[test]
    fn nested_envelopes_belong_to_the_core_loader() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(
            root.path().join("rebaze.toml"),
            "[bazel.build]\nmodule_version = \"2.0\"",
        )
        .unwrap();
        assert_eq!(
            Config::load_from_project(root.path())
                .unwrap()
                .bazel_config()
                .build
                .module_version,
            "2.0"
        );
        assert!(BazelConfig::load_from_project(root.path()).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn broken_config_symlinks_are_errors_not_absence() {
        let root = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink("missing.toml", root.path().join("rebaze.toml")).unwrap();
        assert!(Config::load_from_project(root.path()).is_err());
        assert!(BazelConfig::load_from_project(root.path()).is_err());
    }

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
        assert_eq!(bazel.versions.rules_cc, "0.2.27"); // Default value
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
