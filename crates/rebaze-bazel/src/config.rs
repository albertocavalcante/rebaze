//! Configuration system for rebaze migrations.
//!
//! This module provides a flexible, extensible configuration system that allows
//! users to customize dependency mappings, Bazel versions, filter rules, and more.
//!
//! Configuration can be loaded from:
//! - Default embedded configuration
//! - Project-specific `rebaze.toml` file
//! - CLI overrides

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

/// Main configuration for CMake to Bazel migration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct MigrationConfig {
    /// Bazel dependency versions
    pub versions: VersionConfig,

    /// Dependency mapping configuration
    pub mappings: MappingConfig,

    /// Filter configuration for sources, includes, defines, copts
    pub filters: FilterConfig,

    /// Build file generation options
    pub build: BuildConfig,

    /// Strategy configuration for third-party dependencies
    pub strategy: StrategyConfig,
}

impl Default for MigrationConfig {
    fn default() -> Self {
        Self {
            versions: VersionConfig::default(),
            mappings: MappingConfig::default(),
            filters: FilterConfig::default(),
            build: BuildConfig::default(),
            strategy: StrategyConfig::default(),
        }
    }
}

impl MigrationConfig {
    /// Load configuration from a TOML file, merging with defaults.
    pub fn load_from_file(path: &Path) -> anyhow::Result<Self> {
        let content = std::fs::read_to_string(path)?;
        let config: Self = toml::from_str(&content)?;
        Ok(config)
    }

    /// Try to load configuration from the project directory.
    /// Looks for `rebaze.toml` in the given directory.
    pub fn load_from_project(project_dir: &Path) -> Self {
        let config_path = project_dir.join("rebaze.toml");
        if config_path.exists() {
            match Self::load_from_file(&config_path) {
                Ok(config) => {
                    tracing::info!("Loaded configuration from {}", config_path.display());
                    return config;
                }
                Err(e) => {
                    tracing::warn!(
                        "Failed to load {}: {}. Using defaults.",
                        config_path.display(),
                        e
                    );
                }
            }
        }
        Self::default()
    }

    /// Get the Bazel target for a system library.
    pub fn map_system_library(&self, lib: &str) -> Option<String> {
        // Check explicit mappings first
        if let Some(target) = self.mappings.system_libraries.get(lib) {
            if target.is_empty() {
                return None; // Empty string means "skip this library"
            }
            return Some(target.clone());
        }

        // Check if it's an implicit system library (pthread, m, c, etc.)
        if self.mappings.implicit_system_libs.contains(&lib.to_string()) {
            return None;
        }

        // No mapping found
        None
    }

    /// Get the Bazel target for a CMake imported target (e.g., Boost::filesystem).
    pub fn map_imported_target(&self, package: &str, component: &str) -> Option<String> {
        // Check package-specific mappings
        if let Some(pkg_config) = self.mappings.packages.get(package) {
            // Check component-specific mapping
            if let Some(target) = pkg_config.components.get(component) {
                return Some(target.clone());
            }
            // Use default target pattern with component substitution
            return Some(pkg_config.target_pattern.replace("{component}", component));
        }
        None
    }
}

/// Bazel dependency versions.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct VersionConfig {
    pub rules_cc: String,
    pub platforms: String,
    pub rules_foreign_cc: String,
    pub googletest: String,
    pub google_benchmark: String,
}

impl Default for VersionConfig {
    fn default() -> Self {
        Self {
            rules_cc: "0.1.1".to_string(),
            platforms: "0.0.11".to_string(),
            rules_foreign_cc: "0.14.0".to_string(),
            googletest: "1.15.2".to_string(),
            google_benchmark: "1.9.1".to_string(),
        }
    }
}

/// Dependency mapping configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct MappingConfig {
    /// System library to Bazel target mappings.
    /// Empty string means "skip this library" (implicit system lib).
    pub system_libraries: BTreeMap<String, String>,

    /// Libraries that are implicitly provided by the system and don't need mapping.
    pub implicit_system_libs: Vec<String>,

    /// CMake package configurations.
    pub packages: BTreeMap<String, PackageMapping>,
}

impl Default for MappingConfig {
    fn default() -> Self {
        let mut system_libraries = BTreeMap::new();

        // Compression libraries
        system_libraries.insert("z".to_string(), "@zlib".to_string());
        system_libraries.insert("zlib".to_string(), "@zlib".to_string());
        system_libraries.insert("bz2".to_string(), "@bzip2".to_string());
        system_libraries.insert("bzip2".to_string(), "@bzip2".to_string());
        system_libraries.insert("lzma".to_string(), "@lzma".to_string());
        system_libraries.insert("lz4".to_string(), "@lz4".to_string());
        system_libraries.insert("zstd".to_string(), "@zstd".to_string());

        // Crypto/SSL
        system_libraries.insert("ssl".to_string(), "@openssl".to_string());
        system_libraries.insert("crypto".to_string(), "@openssl".to_string());
        system_libraries.insert("openssl".to_string(), "@openssl".to_string());

        // XML/JSON
        system_libraries.insert("expat".to_string(), "@expat".to_string());
        system_libraries.insert("xml2".to_string(), "@libxml2".to_string());

        // Database
        system_libraries.insert("sqlite3".to_string(), "@sqlite3".to_string());

        // Network
        system_libraries.insert("curl".to_string(), "@curl".to_string());

        // Regex
        system_libraries.insert("pcre".to_string(), "@pcre".to_string());
        system_libraries.insert("pcre2-8".to_string(), "@pcre".to_string());

        // Testing
        system_libraries.insert("gtest".to_string(), "@googletest//:gtest".to_string());
        system_libraries.insert("gtest_main".to_string(), "@googletest//:gtest_main".to_string());
        system_libraries.insert("gmock".to_string(), "@googletest//:gmock".to_string());
        system_libraries.insert("gmock_main".to_string(), "@googletest//:gmock_main".to_string());
        system_libraries.insert("benchmark".to_string(), "@google_benchmark//:benchmark".to_string());
        system_libraries.insert("benchmark_main".to_string(), "@google_benchmark//:benchmark".to_string());

        // Protobuf/gRPC
        system_libraries.insert("protobuf".to_string(), "@com_google_protobuf//:protobuf".to_string());
        system_libraries.insert("grpc".to_string(), "@com_github_grpc_grpc//:grpc".to_string());
        system_libraries.insert("grpc++".to_string(), "@com_github_grpc_grpc//:grpc++".to_string());

        // Implicit system libraries (no Bazel target needed)
        let implicit_system_libs = vec![
            "pthread".to_string(),
            "c".to_string(),
            "m".to_string(),
            "dl".to_string(),
            "rt".to_string(),
            "util".to_string(),
            "resolv".to_string(),
            "nsl".to_string(),
            "socket".to_string(),
        ];

        // Package mappings
        let mut packages = BTreeMap::new();

        packages.insert(
            "Boost".to_string(),
            PackageMapping {
                target_pattern: "@boost//:{component}".to_string(),
                components: BTreeMap::new(),
            },
        );

        packages.insert(
            "OpenSSL".to_string(),
            PackageMapping {
                target_pattern: "@openssl//:{component}".to_string(),
                components: [
                    ("SSL".to_string(), "@openssl//:ssl".to_string()),
                    ("Crypto".to_string(), "@openssl//:crypto".to_string()),
                ]
                .into_iter()
                .collect(),
            },
        );

        packages.insert(
            "ZLIB".to_string(),
            PackageMapping {
                target_pattern: "@zlib".to_string(),
                components: BTreeMap::new(),
            },
        );

        packages.insert(
            "Protobuf".to_string(),
            PackageMapping {
                target_pattern: "@com_google_protobuf//:{component}".to_string(),
                components: [
                    ("protobuf".to_string(), "@com_google_protobuf//:protobuf".to_string()),
                    ("protobuf_lite".to_string(), "@com_google_protobuf//:protobuf_lite".to_string()),
                    ("protoc".to_string(), "@com_google_protobuf//:protoc".to_string()),
                ]
                .into_iter()
                .collect(),
            },
        );

        packages.insert(
            "gRPC".to_string(),
            PackageMapping {
                target_pattern: "@com_github_grpc_grpc//:{component}".to_string(),
                components: [
                    ("grpc".to_string(), "@com_github_grpc_grpc//:grpc".to_string()),
                    ("grpc++".to_string(), "@com_github_grpc_grpc//:grpc++".to_string()),
                ]
                .into_iter()
                .collect(),
            },
        );

        packages.insert(
            "absl".to_string(),
            PackageMapping {
                target_pattern: "@com_google_absl//absl/{component}".to_string(),
                components: BTreeMap::new(),
            },
        );

        packages.insert(
            "GTest".to_string(),
            PackageMapping {
                target_pattern: "@googletest//:{component}".to_string(),
                components: [
                    ("gtest".to_string(), "@googletest//:gtest".to_string()),
                    ("gtest_main".to_string(), "@googletest//:gtest_main".to_string()),
                    ("gmock".to_string(), "@googletest//:gmock".to_string()),
                    ("gmock_main".to_string(), "@googletest//:gmock_main".to_string()),
                ]
                .into_iter()
                .collect(),
            },
        );

        packages.insert(
            "googletest".to_string(),
            PackageMapping {
                target_pattern: "@googletest//:{component}".to_string(),
                components: BTreeMap::new(),
            },
        );

        packages.insert(
            "benchmark".to_string(),
            PackageMapping {
                target_pattern: "@google_benchmark//:{component}".to_string(),
                components: BTreeMap::new(),
            },
        );

        packages.insert(
            "nlohmann_json".to_string(),
            PackageMapping {
                target_pattern: "@nlohmann_json//:json".to_string(),
                components: BTreeMap::new(),
            },
        );

        packages.insert(
            "PkgConfig".to_string(),
            PackageMapping {
                target_pattern: "//third_party:{component}".to_string(),
                components: BTreeMap::new(),
            },
        );

        Self {
            system_libraries,
            implicit_system_libs,
            packages,
        }
    }
}

/// Mapping configuration for a CMake package.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackageMapping {
    /// Default target pattern. Use {component} as placeholder.
    pub target_pattern: String,

    /// Component-specific target overrides.
    #[serde(default)]
    pub components: BTreeMap<String, String>,
}

/// Filter configuration for cleaning up CMake artifacts.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct FilterConfig {
    /// File extensions to exclude from sources.
    pub excluded_source_extensions: Vec<String>,

    /// Define patterns to filter out.
    pub excluded_define_patterns: Vec<String>,

    /// Compiler option patterns to filter out.
    pub excluded_copt_patterns: Vec<String>,

    /// Include path patterns to filter out.
    pub excluded_include_patterns: Vec<String>,
}

impl Default for FilterConfig {
    fn default() -> Self {
        Self {
            excluded_source_extensions: vec![
                // Windows
                "rc".to_string(),
                "res".to_string(),
                "ico".to_string(),
                "manifest".to_string(),
                "def".to_string(),
                // macOS
                "plist".to_string(),
                "xib".to_string(),
                "storyboard".to_string(),
                "xcassets".to_string(),
                // Prebuilt
                "o".to_string(),
                "obj".to_string(),
                "a".to_string(),
                "lib".to_string(),
                "so".to_string(),
                "dylib".to_string(),
                "dll".to_string(),
                // Build system
                "cmake".to_string(),
            ],
            excluded_define_patterns: vec![
                // Windows
                "_WIN32".to_string(),
                "WIN32".to_string(),
                "_WIN64".to_string(),
                "_WINDOWS".to_string(),
                "_MSC_VER".to_string(),
                "NOMINMAX".to_string(),
                // macOS
                "__APPLE__".to_string(),
                "__MACH__".to_string(),
                // Linux
                "__linux__".to_string(),
                "_GNU_SOURCE".to_string(),
                // Debug
                "_DEBUG".to_string(),
                "NDEBUG".to_string(),
            ],
            excluded_copt_patterns: vec![
                // MSVC
                "/".to_string(),
                // Optimization
                "-O".to_string(),
                // Debug
                "-g".to_string(),
                // Standard (handled by toolchain)
                "-std=".to_string(),
                // Architecture
                "-march=".to_string(),
                "-mtune=".to_string(),
            ],
            excluded_include_patterns: vec![
                "/usr/".to_string(),
                "/opt/".to_string(),
                "/Library/".to_string(),
                "/System/".to_string(),
            ],
        }
    }
}

/// Build file generation options.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct BuildConfig {
    /// Default visibility for generated targets.
    pub default_visibility: String,

    /// Header file glob patterns.
    pub header_patterns: Vec<String>,

    /// Default C++ standard.
    pub default_cxx_standard: String,

    /// Default C standard.
    pub default_c_standard: String,

    /// Module version for generated MODULE.bazel.
    pub module_version: String,
}

impl Default for BuildConfig {
    fn default() -> Self {
        Self {
            default_visibility: "//visibility:public".to_string(),
            header_patterns: vec!["**/*.h".to_string(), "**/*.hpp".to_string()],
            default_cxx_standard: "17".to_string(),
            default_c_standard: "11".to_string(),
            module_version: "0.1.0".to_string(),
        }
    }
}

/// Third-party dependency strategy configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct StrategyConfig {
    /// Default strategy for resolving dependencies.
    pub default: String,

    /// Per-package strategy overrides.
    #[serde(default)]
    pub overrides: BTreeMap<String, String>,
}

impl Default for StrategyConfig {
    fn default() -> Self {
        Self {
            default: "system".to_string(),
            overrides: BTreeMap::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = MigrationConfig::default();
        assert_eq!(config.versions.rules_cc, "0.1.1");
        assert_eq!(config.strategy.default, "system");
    }

    #[test]
    fn test_system_library_mapping() {
        let config = MigrationConfig::default();

        // Known library
        assert_eq!(
            config.map_system_library("zlib"),
            Some("@zlib".to_string())
        );

        // Implicit system library
        assert_eq!(config.map_system_library("pthread"), None);

        // Unknown library
        assert_eq!(config.map_system_library("unknown_lib"), None);
    }

    #[test]
    fn test_imported_target_mapping() {
        let config = MigrationConfig::default();

        // OpenSSL with component
        assert_eq!(
            config.map_imported_target("OpenSSL", "SSL"),
            Some("@openssl//:ssl".to_string())
        );

        // GTest
        assert_eq!(
            config.map_imported_target("GTest", "gtest"),
            Some("@googletest//:gtest".to_string())
        );

        // Boost with dynamic component
        assert_eq!(
            config.map_imported_target("Boost", "filesystem"),
            Some("@boost//:filesystem".to_string())
        );
    }

    #[test]
    fn test_toml_parsing() {
        let toml_content = r#"
[versions]
rules_cc = "0.3.0"
googletest = "1.16.0"

[mappings.system_libraries]
mylib = "@my_repo//:mylib"

[strategy]
default = "source"
"#;
        let config: MigrationConfig = toml::from_str(toml_content).unwrap();
        assert_eq!(config.versions.rules_cc, "0.3.0");
        assert_eq!(config.versions.googletest, "1.16.0");
        assert_eq!(config.strategy.default, "source");
        assert_eq!(
            config.mappings.system_libraries.get("mylib"),
            Some(&"@my_repo//:mylib".to_string())
        );
    }
}
