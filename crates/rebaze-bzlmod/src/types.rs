//! Core types for module resolution.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::label::{ModuleName, Version};

/// Information parsed from a MODULE.bazel file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModuleInfo {
    /// Module name.
    pub name: ModuleName,
    /// Module version.
    pub version: Version,
    /// Compatibility level.
    #[serde(default)]
    pub compatibility_level: u32,
    /// Bazel compatibility (minimum bazel version).
    #[serde(default)]
    pub bazel_compatibility: Vec<String>,
    /// Direct dependencies.
    #[serde(default)]
    pub deps: Vec<Dependency>,
    /// Development dependencies.
    #[serde(default)]
    pub dev_deps: Vec<Dependency>,
    /// Overrides.
    #[serde(default)]
    pub overrides: Vec<Override>,
}

/// A `bazel_dep` dependency declaration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Dependency {
    /// Module name.
    pub name: ModuleName,
    /// Version constraint.
    pub version: Version,
    /// Maximum version (for version ranges).
    #[serde(default)]
    pub max_version: Option<Version>,
    /// Repository name override.
    #[serde(default)]
    pub repo_name: Option<String>,
    /// Whether this is a dev dependency.
    #[serde(default)]
    pub dev_dependency: bool,
}

/// Module override configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Override {
    /// Pin to a single version.
    SingleVersion {
        /// Module name being overridden.
        module: ModuleName,
        /// Pinned version.
        version: Version,
        /// Optional registry URL.
        registry: Option<String>,
    },
    /// Override with git repository.
    Git {
        /// Module name being overridden.
        module: ModuleName,
        /// Git remote URL.
        remote: String,
        /// Git commit SHA.
        commit: Option<String>,
        /// Git tag.
        tag: Option<String>,
        /// Git branch.
        branch: Option<String>,
    },
    /// Override with local path.
    LocalPath {
        /// Module name being overridden.
        module: ModuleName,
        /// Local filesystem path.
        path: String,
    },
    /// Override with archive.
    Archive {
        /// Module name being overridden.
        module: ModuleName,
        /// Archive URLs.
        urls: Vec<String>,
        /// Archive integrity hash.
        integrity: Option<String>,
        /// Strip prefix from archive.
        strip_prefix: Option<String>,
    },
    /// Allow multiple versions.
    MultipleVersion {
        /// Module name being overridden.
        module: ModuleName,
        /// Allowed versions.
        versions: Vec<Version>,
        /// Optional registry URL.
        registry: Option<String>,
    },
}

/// A resolved module in the dependency graph.
// Multiple boolean flags are needed to represent various module states from different sources
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvedModule {
    /// Module name.
    pub name: ModuleName,
    /// Selected version.
    pub version: Version,
    /// Registry this module came from.
    pub registry: String,
    /// Whether this is a direct dependency.
    pub is_direct: bool,
    /// Whether this is a dev dependency.
    pub is_dev: bool,
    /// Compatibility level.
    pub compatibility_level: u32,
    /// Whether this version is yanked.
    #[serde(default)]
    pub yanked: bool,
    /// Yanked reason if applicable.
    #[serde(default)]
    pub yanked_reason: Option<String>,
    /// Whether this module is deprecated.
    #[serde(default)]
    pub deprecated: bool,
    /// Direct dependencies of this module.
    #[serde(default)]
    pub deps: Vec<ModuleKey>,
}

/// Unique key for a module (name + version).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ModuleKey {
    /// Module name.
    pub name: ModuleName,
    /// Module version.
    pub version: Version,
}

impl ModuleKey {
    /// Create a new module key.
    #[must_use]
    pub const fn new(name: ModuleName, version: Version) -> Self {
        Self { name, version }
    }
}

impl std::fmt::Display for ModuleKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}@{}", self.name, self.version)
    }
}

/// Module metadata from registry.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ModuleMetadata {
    /// Available versions.
    #[serde(default)]
    pub versions: Vec<String>,
    /// Yanked versions with reasons.
    #[serde(default)]
    pub yanked_versions: BTreeMap<String, String>,
    /// Deprecated message if module is deprecated.
    #[serde(default)]
    pub deprecated: Option<String>,
    /// Module maintainers.
    #[serde(default)]
    pub maintainers: Vec<Maintainer>,
    /// Homepage URL.
    #[serde(default)]
    pub homepage: Option<String>,
}

/// Module maintainer information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Maintainer {
    /// Maintainer name.
    #[serde(default)]
    pub name: Option<String>,
    /// Maintainer email.
    #[serde(default)]
    pub email: Option<String>,
    /// Maintainer GitHub handle.
    #[serde(default)]
    pub github: Option<String>,
}

/// Source information for a module version.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceInfo {
    /// Source type (archive, git, local).
    #[serde(rename = "type", default)]
    pub source_type: String,
    /// Archive URLs.
    #[serde(default)]
    pub url: Option<String>,
    /// Archive integrity hash.
    #[serde(default)]
    pub integrity: Option<String>,
    /// Strip prefix for archive.
    #[serde(default)]
    pub strip_prefix: Option<String>,
    /// Patches to apply.
    #[serde(default)]
    pub patches: BTreeMap<String, String>,
    /// Patch strip level.
    #[serde(default)]
    pub patch_strip: u32,
}
