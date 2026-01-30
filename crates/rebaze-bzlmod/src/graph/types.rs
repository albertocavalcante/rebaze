//! Types for dependency graph representation and querying.
//!
//! Contains data structures for explaining version selections,
//! tracking dependency chains, and exporting graphs.

use crate::ModuleKey;
use serde::{Deserialize, Serialize};

/// Explanation for why a module version was selected.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Explanation {
    /// The module being explained.
    pub module: String,
    /// Selected version.
    pub version: String,
    /// Paths that requested this module.
    pub requesters: Vec<RequesterInfo>,
    /// Why this specific version was chosen.
    pub reason: String,
    /// Selection information.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selection: Option<SelectionInfo>,
    /// All dependency chains from root to this module.
    pub dependency_chains: Vec<DependencyChain>,
}

/// Information about a module that requested a dependency.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequesterInfo {
    /// Requester module key.
    pub requester: ModuleKey,
    /// Version requested.
    pub requested_version: String,
    /// Path from root to requester.
    pub path: Vec<ModuleKey>,
}

/// Information about how a version was selected.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SelectionInfo {
    /// Strategy used for selection.
    pub strategy: SelectionStrategy,
    /// The version that was selected.
    pub selected_version: String,
    /// All versions that were considered.
    pub candidates: Vec<VersionCandidate>,
    /// What determined the selection.
    pub deciding_factor: String,
}

/// Strategy used for version selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SelectionStrategy {
    /// Version was selected by Minimal Version Selection.
    Mvs,
    /// Version was forced by an override.
    Override,
    /// A `single_version_override` was applied.
    SingleVersionOverride,
    /// This is the root module (no selection needed).
    Root,
}

impl std::fmt::Display for SelectionStrategy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Mvs => write!(f, "mvs"),
            Self::Override => write!(f, "override"),
            Self::SingleVersionOverride => write!(f, "single_version_override"),
            Self::Root => write!(f, "root"),
        }
    }
}

/// A version candidate considered during selection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VersionCandidate {
    /// The version string.
    pub version: String,
    /// Modules that requested this version.
    pub requested_by: Vec<ModuleKey>,
    /// Whether this version was selected.
    pub selected: bool,
    /// Why this version was not selected (if applicable).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rejection_reason: Option<String>,
}

/// A path of dependencies from root to a module.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DependencyChain {
    /// The sequence of modules from root to target.
    pub path: Vec<ModuleKey>,
    /// The version requested at the end of this chain.
    pub requested_version: String,
}

impl std::fmt::Display for DependencyChain {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.path.is_empty() {
            return Ok(());
        }

        let path_str: Vec<_> = self.path.iter().map(ToString::to_string).collect();
        write!(f, "{}", path_str.join(" -> "))?;

        if !self.requested_version.is_empty() {
            write!(f, " (requested {})", self.requested_version)?;
        }

        Ok(())
    }
}

/// Statistics about the dependency graph.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GraphStats {
    /// Total number of modules in the graph.
    pub total_modules: usize,
    /// Number of direct dependencies of the root.
    pub direct_dependencies: usize,
    /// Number of transitive dependencies.
    pub transitive_dependencies: usize,
    /// Maximum depth of the dependency tree.
    pub max_depth: usize,
    /// Number of dev-only dependencies.
    pub dev_dependencies: usize,
}

/// Entry in the flat module list.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModuleListEntry {
    /// Module name.
    pub name: String,
    /// Module version.
    pub version: String,
    /// Whether this is a dev dependency.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub dev_dependency: bool,
    /// Modules that require this one.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub required_by: Vec<String>,
}

/// Bazel's mod graph JSON output structure.
/// Matches the output of `bazel mod graph --output=json`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BazelModGraph {
    /// Module key (name@version).
    pub key: String,
    /// Module name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Module version.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// Direct dependencies.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dependencies: Vec<BazelDependency>,
    /// Indirect dependencies.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub indirect_dependencies: Vec<BazelDependency>,
    /// Cycles in the graph.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cycles: Vec<BazelDependency>,
    /// Whether this is the root module.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub root: bool,
}

/// A dependency in Bazel's module graph format.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BazelDependency {
    /// Module key (name@version).
    pub key: String,
    /// Direct dependencies.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dependencies: Vec<BazelDependency>,
    /// Indirect dependencies.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub indirect_dependencies: Vec<BazelDependency>,
    /// Cycles involving this dependency.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cycles: Vec<BazelDependency>,
    /// Whether this node is unexpanded (to avoid infinite recursion).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub unexpanded: bool,
}
