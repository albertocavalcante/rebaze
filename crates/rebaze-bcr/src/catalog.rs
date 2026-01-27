//! BCR catalog data structures.
//!
//! This module defines the core types for representing BCR modules,
//! their targets, and package name mappings.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A module available in the Bazel Central Registry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BcrModule {
    /// Module name as registered in BCR (e.g., "protobuf")
    pub name: String,

    /// Available versions in the registry
    pub versions: Vec<String>,

    /// Latest stable version
    pub latest_version: String,

    /// Minimum Bazel version required (if any)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_bazel_version: Option<String>,

    /// Dependencies of this module (module name -> version constraint)
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub dependencies: BTreeMap<String, String>,

    /// Exported targets from this module
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub targets: Vec<BcrTarget>,
}

/// A target exported by a BCR module.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BcrTarget {
    /// Target name (e.g., "protobuf_lite")
    pub name: String,

    /// Full Bazel label (e.g., "@protobuf//:protobuf_lite")
    pub label: String,

    /// Target type (e.g., "cc_library", "java_library")
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub target_type: String,
}

/// Confidence level for a package mapping.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum MappingConfidence {
    /// Exact match - official CMake/Gradle name maps to BCR module
    Exact,
    /// Likely match - naming convention suggests this mapping
    #[default]
    Likely,
    /// Guess - heuristic-based mapping that may need verification
    Guess,
}

/// Mapping from a CMake/Gradle package name to a BCR module.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackageMapping {
    /// BCR module name
    pub module: String,

    /// Default target to use (if not the module name)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,

    /// Component to target mappings (for packages like Boost)
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub components: BTreeMap<String, String>,

    /// Confidence level of this mapping
    #[serde(default)]
    pub confidence: MappingConfidence,
}

impl PackageMapping {
    /// Create a new mapping with exact confidence.
    #[must_use]
    pub fn exact(module: impl Into<String>) -> Self {
        Self {
            module: module.into(),
            target: None,
            components: BTreeMap::new(),
            confidence: MappingConfidence::Exact,
        }
    }

    /// Create a new mapping with exact confidence and a specific target.
    #[must_use]
    pub fn exact_with_target(module: impl Into<String>, target: impl Into<String>) -> Self {
        Self {
            module: module.into(),
            target: Some(target.into()),
            components: BTreeMap::new(),
            confidence: MappingConfidence::Exact,
        }
    }

    /// Set components for this mapping.
    #[must_use]
    pub fn with_components(mut self, components: BTreeMap<String, String>) -> Self {
        self.components = components;
        self
    }
}

/// The complete BCR catalog containing modules and mappings.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BcrCatalog {
    /// BCR modules indexed by name
    #[serde(default)]
    pub modules: BTreeMap<String, BcrModule>,

    /// CMake package name to BCR module mappings
    #[serde(default)]
    pub cmake_mappings: BTreeMap<String, PackageMapping>,

    /// Gradle artifact to BCR module mappings
    #[serde(default)]
    pub gradle_mappings: BTreeMap<String, PackageMapping>,
}

impl BcrCatalog {
    /// Create a new empty catalog.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Get a module by name.
    #[must_use]
    pub fn get_module(&self, name: &str) -> Option<&BcrModule> {
        self.modules.get(name)
    }

    /// Get the latest version of a module.
    #[must_use]
    pub fn get_latest_version(&self, module_name: &str) -> Option<&str> {
        self.modules
            .get(module_name)
            .map(|m| m.latest_version.as_str())
    }

    /// Check if a module exists in the catalog.
    #[must_use]
    pub fn has_module(&self, name: &str) -> bool {
        self.modules.contains_key(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_package_mapping_exact() {
        let mapping = PackageMapping::exact("protobuf");
        assert_eq!(mapping.module, "protobuf");
        assert_eq!(mapping.confidence, MappingConfidence::Exact);
        assert!(mapping.target.is_none());
    }

    #[test]
    fn test_package_mapping_with_target() {
        let mapping = PackageMapping::exact_with_target("protobuf", "@protobuf//:protobuf");
        assert_eq!(mapping.module, "protobuf");
        assert_eq!(mapping.target, Some("@protobuf//:protobuf".to_string()));
    }

    #[test]
    fn test_catalog_get_module() {
        let mut catalog = BcrCatalog::new();
        catalog.modules.insert(
            "test".to_string(),
            BcrModule {
                name: "test".to_string(),
                versions: vec!["1.0.0".to_string()],
                latest_version: "1.0.0".to_string(),
                min_bazel_version: None,
                dependencies: BTreeMap::new(),
                targets: vec![],
            },
        );

        assert!(catalog.has_module("test"));
        assert!(!catalog.has_module("nonexistent"));
        assert_eq!(catalog.get_latest_version("test"), Some("1.0.0"));
    }
}
