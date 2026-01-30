//! Types for dependency resolution.

use std::collections::HashSet;

use crate::graph::DependencyGraph;
use crate::{ModuleInfo, ResolvedModule};

/// Default maximum concurrent fetches from registry.
pub const DEFAULT_MAX_CONCURRENCY: usize = 5;

/// Default maximum dependency depth.
pub const DEFAULT_MAX_DEPTH: usize = 1000;

/// Default request timeout in seconds.
pub const DEFAULT_TIMEOUT_SECS: u64 = 30;

/// Options for dependency resolution.
#[derive(Debug, Clone)]
pub struct ResolverOptions {
    /// Registry URLs (in order of preference).
    pub registries: Vec<String>,
    /// Include dev dependencies.
    pub include_dev_deps: bool,
    /// Check for yanked versions.
    pub check_yanked: bool,
    /// Behavior when encountering yanked versions.
    pub yanked_behavior: YankedBehavior,
    /// Allow specific yanked versions (module@version).
    pub allow_yanked: HashSet<String>,
    /// Whether to substitute yanked versions with non-yanked alternatives.
    pub substitute_yanked: bool,
    /// Maximum resolution depth.
    pub max_depth: usize,
    /// Number of concurrent fetches.
    pub concurrency: usize,
    /// Request timeout in seconds.
    pub timeout_secs: u64,
    /// Direct dependency validation mode.
    pub direct_deps_mode: DirectDepsMode,
}

impl Default for ResolverOptions {
    fn default() -> Self {
        Self {
            registries: vec![crate::DEFAULT_REGISTRY.to_string()],
            include_dev_deps: false,
            check_yanked: true,
            yanked_behavior: YankedBehavior::Warn,
            allow_yanked: HashSet::new(),
            substitute_yanked: false,
            max_depth: DEFAULT_MAX_DEPTH,
            concurrency: DEFAULT_MAX_CONCURRENCY,
            timeout_secs: DEFAULT_TIMEOUT_SECS,
            direct_deps_mode: DirectDepsMode::Off,
        }
    }
}

/// Behavior when encountering yanked versions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum YankedBehavior {
    /// Allow yanked versions without warnings.
    Allow,
    /// Warn about yanked versions but continue.
    #[default]
    Warn,
    /// Error when encountering yanked versions.
    Error,
}

/// Mode for validating direct dependencies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DirectDepsMode {
    /// Don't validate direct dependencies.
    #[default]
    Off,
    /// Warn when direct dependency versions don't match resolved.
    Warn,
    /// Error when direct dependency versions don't match resolved.
    Error,
}

/// Result of dependency resolution.
#[derive(Debug, Clone)]
pub struct ResolutionResult {
    /// Root module information.
    pub root: ModuleInfo,
    /// All resolved modules (sorted by name).
    pub modules: Vec<ResolvedModule>,
    /// Dependency graph.
    pub graph: DependencyGraph,
    /// Resolution statistics.
    pub stats: ResolutionStats,
    /// Warnings generated during resolution.
    pub warnings: Vec<String>,
}

/// Statistics about the resolution.
#[derive(Debug, Clone, Default)]
pub struct ResolutionStats {
    /// Total modules resolved.
    pub total_modules: usize,
    /// Direct dependencies.
    pub direct_deps: usize,
    /// Transitive dependencies.
    pub transitive_deps: usize,
    /// Dev dependencies included.
    pub dev_deps: usize,
    /// Yanked versions encountered.
    pub yanked_versions: usize,
    /// Deprecated modules encountered.
    pub deprecated_modules: usize,
    /// Registry fetch count.
    pub registry_fetches: usize,
    /// Cache hits.
    pub cache_hits: usize,
}
