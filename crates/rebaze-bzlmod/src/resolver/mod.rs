//! Dependency resolution using MVS algorithm.
//!
//! This module implements Bazel's Minimal Version Selection (MVS) algorithm
//! for resolving module dependencies. The algorithm proceeds in three phases:
//!
//! 1. **Graph construction**: Recursively fetches MODULE.bazel files from the registry
//!    to build a complete dependency graph with all requested versions.
//! 2. **Override application**: Applies `single_version` overrides to pin versions and
//!    preserves `git`/`local_path`/`archive` overrides without fetching from the registry.
//! 3. **MVS selection**: For each module, selects the highest version requested by
//!    any dependent module.
//!
//! The resolver fetches dependencies concurrently and caches results to avoid
//! redundant network requests.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::RwLock;
use tokio::task::JoinSet;
use tracing::info;

use crate::graph::DependencyGraph;
use crate::label::{ModuleName, Version};
use crate::registry::Registry;
use crate::{Dependency, Error, ModuleInfo, ModuleKey, Override, ResolvedModule, Result};

mod types;
mod version;

// Re-export public types
pub use types::{
    DEFAULT_MAX_CONCURRENCY, DEFAULT_MAX_DEPTH, DEFAULT_TIMEOUT_SECS, DirectDepsMode,
    ResolutionResult, ResolutionStats, ResolverOptions, YankedBehavior,
};
pub use version::compare_versions;

/// A dependency request with metadata.
#[derive(Debug, Clone)]
struct DepRequest {
    /// Requested version.
    version: Version,
    /// Whether this is a dev dependency.
    dev_dependency: bool,
    /// Modules that requested this dependency.
    required_by: Vec<String>,
}

/// Mutable graph state belongs to the coordinator; fetch tasks only return data.
struct GraphBuildContext {
    root_name: ModuleName,
    requests: HashMap<String, HashMap<String, DepRequest>>,
    modules: HashMap<ModuleKey, ModuleInfo>,
    queued: HashSet<ModuleKey>,
    overrides: HashMap<String, Override>,
    override_modules: HashMap<String, ModuleInfo>,
}

impl GraphBuildContext {
    fn new(root: &ModuleInfo, override_modules: HashMap<String, ModuleInfo>) -> Self {
        let overrides = root
            .overrides
            .iter()
            .map(|override_| {
                let (Override::SingleVersion { module, .. }
                | Override::Git { module, .. }
                | Override::LocalPath { module, .. }
                | Override::Archive { module, .. }
                | Override::MultipleVersion { module, .. }) = override_;
                (module.as_str().to_owned(), override_.clone())
            })
            .collect();
        Self {
            root_name: root.name.clone(),
            requests: HashMap::new(),
            modules: HashMap::new(),
            queued: HashSet::new(),
            overrides,
            override_modules,
        }
    }
}

/// Dependency resolver using MVS algorithm.
pub struct Resolver<R: Registry> {
    registry: Arc<R>,
    options: ResolverOptions,
    override_modules: Arc<RwLock<HashMap<String, ModuleInfo>>>,
}

impl<R: Registry + 'static> Resolver<R> {
    /// Create a new resolver with a registry and options.
    pub fn new(registry: R, options: ResolverOptions) -> Self {
        Self {
            registry: Arc::new(registry),
            options,
            override_modules: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Add MODULE.bazel content for a git/local/archive override.
    ///
    /// The content is parsed and used to hydrate transitive dependencies for that module.
    pub async fn add_override_module_content(
        &self,
        module_name: &str,
        content: &str,
    ) -> Result<()> {
        if module_name.is_empty() {
            return Err(Error::Resolution(
                "override module name is empty".to_string(),
            ));
        }

        // Parse the MODULE.bazel content
        let module_info = parse_module_content(content)?;
        self.add_override_module_info(module_name, module_info)
            .await
    }

    /// Add parsed module info for a git/local/archive override.
    pub async fn add_override_module_info(
        &self,
        module_name: &str,
        mut module_info: ModuleInfo,
    ) -> Result<()> {
        if module_name.is_empty() {
            return Err(Error::Resolution(
                "override module name is empty".to_string(),
            ));
        }

        // Ensure the module name matches
        if module_info.name.as_str().is_empty() {
            module_info.name = ModuleName::new(module_name)?;
        } else if module_info.name.as_str() != module_name {
            return Err(Error::Resolution(format!(
                "override module name mismatch: {} != {}",
                module_info.name, module_name
            )));
        }

        let mut override_modules = self.override_modules.write().await;
        override_modules.insert(module_name.to_string(), module_info);
        Ok(())
    }

    /// Resolve dependencies for a root module.
    ///
    /// This is the main entry point for dependency resolution.
    pub async fn resolve_dependencies(&self, root_module: &ModuleInfo) -> Result<ResolutionResult> {
        info!(
            "Resolving dependencies for {}@{}",
            root_module.name, root_module.version
        );

        // Clone the override modules snapshot
        let override_modules = self.override_modules.read().await.clone();

        // Initialize graph build context
        if self.options.concurrency == 0 {
            return Err(Error::Resolution(
                "concurrency must be greater than zero".into(),
            ));
        }
        for override_ in &root_module.overrides {
            match override_ {
                Override::MultipleVersion { .. } => {
                    return Err(Error::Resolution(
                        "multiple-version override resolution is not supported".into(),
                    ));
                }
                Override::SingleVersion {
                    registry: Some(_), ..
                } => {
                    return Err(Error::Resolution(
                        "registry-specific override resolution is not supported".into(),
                    ));
                }
                _ => {}
            }
        }
        let mut ctx = GraphBuildContext::new(root_module, override_modules);
        let mut stats = ResolutionStats::default();
        self.build_dependency_graph(root_module, &mut ctx, &mut stats)
            .await?;

        let selected_versions = Self::apply_mvs(&ctx.requests);

        // Validate direct dependencies if enabled
        let mut warnings = Vec::new();
        if self.options.direct_deps_mode != DirectDepsMode::Off {
            let mismatches = self.check_direct_deps(root_module, &selected_versions);
            if !mismatches.is_empty() {
                if self.options.direct_deps_mode == DirectDepsMode::Error {
                    let msg = mismatches
                        .iter()
                        .map(|(name, declared, resolved)| {
                            format!("{name}: declared {declared} but resolved {resolved}")
                        })
                        .collect::<Vec<_>>()
                        .join(", ");
                    return Err(Error::Resolution(format!(
                        "direct dependency version mismatch: {msg}"
                    )));
                }
                // DirectDepsMode::Warn
                for (name, declared, resolved) in mismatches {
                    warnings.push(format!(
                        "direct dependency {name} declared as {declared} but resolved to {resolved}"
                    ));
                }
            }
        }

        // Build the resolution result
        let (modules, graph, more_warnings) = self
            .build_resolution_result(&selected_versions, &ctx, root_module, &mut stats)
            .await?;

        warnings.extend(more_warnings);

        // Update stats
        stats.total_modules = modules.len();
        stats.direct_deps = modules.iter().filter(|module| module.is_direct).count();
        stats.transitive_deps = stats.total_modules.saturating_sub(stats.direct_deps);

        Ok(ResolutionResult {
            root: root_module.clone(),
            modules,
            graph,
            stats,
            warnings,
        })
    }

    /// Fetch each requested module once, with all graph mutations on this task.
    /// Dropping the JoinSet on error or cancellation aborts outstanding fetches.
    async fn build_dependency_graph(
        &self,
        root: &ModuleInfo,
        ctx: &mut GraphBuildContext,
        stats: &mut ResolutionStats,
    ) -> Result<()> {
        let mut pending = VecDeque::new();
        let mut next_layer = VecDeque::new();
        let mut fetches = JoinSet::new();
        self.enqueue_dependencies(root, 0, true, ctx, &mut pending)
            .await?;

        loop {
            // Finish every fetch at this depth before scheduling the next layer.
            // Depth checks then depend on shortest paths, not network completion order.
            if pending.is_empty() && fetches.is_empty() {
                if next_layer.is_empty() {
                    break;
                }
                std::mem::swap(&mut pending, &mut next_layer);
            }
            while fetches.len() < self.options.concurrency {
                let Some((key, depth)) = pending.pop_front() else {
                    break;
                };
                let external_override = matches!(
                    ctx.overrides.get(key.name.as_str()),
                    Some(
                        Override::Git { .. }
                            | Override::LocalPath { .. }
                            | Override::Archive { .. }
                    )
                );
                if external_override {
                    let module = ctx.override_modules.get(key.name.as_str()).cloned()
                        .ok_or_else(|| Error::Resolution(format!("override for {} requires MODULE.bazel content; call add_override_module_content first", key.name)))?;
                    self.enqueue_dependencies(&module, depth, false, ctx, &mut next_layer)
                        .await?;
                    ctx.modules.insert(key, module);
                    continue;
                }

                let registry = Arc::clone(&self.registry);
                let timeout = Duration::from_secs(self.options.timeout_secs);
                fetches.spawn(async move {
                    let module = tokio::time::timeout(
                        timeout,
                        registry.get_module(key.name.as_str(), key.version.as_str()),
                    )
                    .await
                    .map_err(|_| Error::Resolution(format!("fetch {key} timed out")))??;
                    Ok::<_, Error>((key, depth, module))
                });
            }

            let Some(result) = fetches.join_next().await else {
                continue;
            };
            let (key, depth, module) = result.map_err(|error| {
                Error::Resolution(format!("module fetch task failed: {error}"))
            })??;
            stats.registry_fetches += 1;
            self.enqueue_dependencies(&module, depth, false, ctx, &mut next_layer)
                .await?;
            ctx.modules.insert(key, module);
        }
        Ok(())
    }

    /// Apply root overrides and schedule previously unseen requests.
    async fn enqueue_dependencies(
        &self,
        module: &ModuleInfo,
        depth: usize,
        is_root: bool,
        ctx: &mut GraphBuildContext,
        pending: &mut VecDeque<(ModuleKey, usize)>,
    ) -> Result<()> {
        for (dep, is_dev) in dependencies(module, is_root && self.options.include_dev_deps) {
            // The root module wins by name, even when a dependency requests another version.
            if dep.name == ctx.root_name {
                continue;
            }
            let override_ = ctx.overrides.get(dep.name.as_str());
            let mut version = match override_ {
                Some(Override::SingleVersion { version, .. }) => version.clone(),
                _ => dep.version.clone(),
            };
            // Fetch the replacement's MODULE.bazel too, so its dependencies are included.
            if self.options.substitute_yanked
                && override_.is_none()
                && !self
                    .options
                    .allow_yanked
                    .contains(&format!("{}@{}", dep.name, version))
                && let Some(replacement) = self
                    .find_non_yanked_version(dep.name.as_str(), version.as_str())
                    .await?
            {
                version = Version::new(replacement)?;
            }
            let request = ctx
                .requests
                .entry(dep.name.as_str().to_owned())
                .or_default()
                .entry(version.as_str().to_owned())
                .or_insert_with(|| DepRequest {
                    version: version.clone(),
                    dev_dependency: is_dev,
                    required_by: Vec::new(),
                });
            request.dev_dependency &= is_dev;
            request.required_by.push(module.name.as_str().to_owned());

            let key = ModuleKey::new(dep.name.clone(), version);
            if ctx.queued.insert(key.clone()) {
                if depth >= self.options.max_depth {
                    return Err(Error::MaxDepthExceeded(self.options.max_depth));
                }
                pending.push_back((key, depth + 1));
            }
        }
        Ok(())
    }

    /// Apply MVS: select the highest version for each module.
    fn apply_mvs(
        dep_graph: &HashMap<String, HashMap<String, DepRequest>>,
    ) -> HashMap<String, DepRequest> {
        let mut selected = HashMap::new();

        for (module_name, versions) in dep_graph {
            if let Some(req) = versions
                .values()
                .max_by(|a, b| compare_versions(a.version.as_str(), b.version.as_str()).cmp(&0))
            {
                let mut selected_request = req.clone();
                selected_request.dev_dependency =
                    versions.values().all(|request| request.dev_dependency);
                selected.insert(module_name.clone(), selected_request);
            }
        }

        selected
    }

    /// Check that direct dependency versions match resolved versions.
    fn check_direct_deps(
        &self,
        root_module: &ModuleInfo,
        selected: &HashMap<String, DepRequest>,
    ) -> Vec<(String, String, String)> {
        let mut mismatches = Vec::new();

        let all_deps: Vec<&Dependency> = if self.options.include_dev_deps {
            root_module
                .deps
                .iter()
                .chain(root_module.dev_deps.iter())
                .collect()
        } else {
            root_module.deps.iter().collect()
        };

        for dep in all_deps {
            let dep_name = dep.name.as_str();
            if let Some(resolved) = selected.get(dep_name)
                && resolved.version.as_str() != dep.version.as_str()
            {
                mismatches.push((
                    dep_name.to_string(),
                    dep.version.as_str().to_string(),
                    resolved.version.as_str().to_string(),
                ));
            }
        }

        mismatches
    }

    /// Build the final resolution result.
    async fn build_resolution_result(
        &self,
        selected_versions: &HashMap<String, DepRequest>,
        ctx: &GraphBuildContext,
        root_module: &ModuleInfo,
        stats: &mut ResolutionStats,
    ) -> Result<(Vec<ResolvedModule>, DependencyGraph, Vec<String>)> {
        let fetched_modules = &ctx.modules;
        let mut modules = Vec::new();
        let root_key = ModuleKey::new(root_module.name.clone(), root_module.version.clone());
        let mut graph = DependencyGraph::with_root(root_key.clone());
        let mut warnings = Vec::new();

        let direct_deps: HashSet<&str> = dependencies(root_module, self.options.include_dev_deps)
            .map(|(dep, _)| dep.name.as_str())
            .collect();

        // Only dependencies reachable through selected versions belong in the result.
        let mut reachable = HashSet::new();
        let mut pending: VecDeque<&str> = direct_deps.iter().copied().collect();
        while let Some(name) = pending.pop_front() {
            if !reachable.insert(name) {
                continue;
            }
            if let Some(request) = selected_versions.get(name) {
                let key = ModuleKey::new(ModuleName::new(name)?, request.version.clone());
                if let Some(module) = fetched_modules.get(&key) {
                    pending.extend(dependencies(module, false).map(|(dep, _)| dep.name.as_str()));
                }
            }
        }

        for (module_name, req) in selected_versions {
            if !reachable.contains(module_name.as_str()) {
                continue;
            }
            let is_direct = direct_deps.contains(module_name.as_str());
            let key = ModuleKey::new(ModuleName::new(module_name.clone())?, req.version.clone());
            let info = fetched_modules.get(&key);
            let deps = info
                .map(|module| {
                    dependencies(module, false)
                        .filter_map(|(dep, _)| {
                            if dep.name == root_module.name {
                                Some(root_key.clone())
                            } else {
                                selected_versions.get(dep.name.as_str()).map(|request| {
                                    ModuleKey::new(dep.name.clone(), request.version.clone())
                                })
                            }
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();

            let resolved = ResolvedModule {
                name: ModuleName::new(module_name.clone())?,
                version: req.version.clone(),
                registry: self
                    .options
                    .registries
                    .first()
                    .cloned()
                    .unwrap_or_else(|| crate::DEFAULT_REGISTRY.to_string()),
                is_direct,
                is_dev: req.dev_dependency,
                compatibility_level: info.map_or(0, |module| module.compatibility_level),
                yanked: false,
                yanked_reason: None,
                deprecated: false,
                deps: deps.clone(),
            };

            modules.push(resolved);

            if req.dev_dependency {
                stats.dev_deps += 1;
            }
        }

        // Sort modules by name
        modules.sort_by(|a, b| a.name.as_str().cmp(b.name.as_str()));

        // Check for yanked versions if enabled
        if self.options.check_yanked {
            for module in &mut modules {
                if matches!(
                    ctx.overrides.get(module.name.as_str()),
                    Some(
                        Override::Git { .. }
                            | Override::LocalPath { .. }
                            | Override::Archive { .. }
                    )
                ) {
                    continue;
                }
                match self.fetch_metadata(module.name.as_str()).await {
                    Ok(metadata) => {
                        if let Some(reason) = metadata.yanked_versions.get(module.version.as_str())
                        {
                            module.yanked = true;
                            module.yanked_reason = Some(reason.clone());
                            stats.yanked_versions += 1;

                            let module_key =
                                format!("{}@{}", module.name.as_str(), module.version.as_str());
                            if !self.options.allow_yanked.contains(&module_key) {
                                match self.options.yanked_behavior {
                                    YankedBehavior::Allow => {}
                                    YankedBehavior::Warn => {
                                        warnings.push(format!(
                                            "module {}@{} is yanked: {}",
                                            module.name, module.version, reason
                                        ));
                                    }
                                    YankedBehavior::Error => {
                                        return Err(Error::YankedVersion {
                                            module: module.name.as_str().to_string(),
                                            version: module.version.as_str().to_string(),
                                            reason: reason.clone(),
                                        });
                                    }
                                }
                            }
                        }

                        if metadata.deprecated.is_some() {
                            module.deprecated = true;
                            stats.deprecated_modules += 1;
                            if let Some(msg) = &metadata.deprecated {
                                warnings
                                    .push(format!("module {} is deprecated: {}", module.name, msg));
                            }
                        }
                    }
                    Err(error) => return Err(error),
                }
            }
        }

        // Populate the root and nodes after metadata updates. Sorted modules also
        // make root edges independent of HashMap iteration and fetch completion.
        let root_deps: Vec<_> = modules
            .iter()
            .filter(|module| module.is_direct)
            .map(|module| ModuleKey::new(module.name.clone(), module.version.clone()))
            .collect();
        graph.add_module(ResolvedModule {
            name: root_module.name.clone(),
            version: root_module.version.clone(),
            registry: String::new(),
            is_direct: false,
            is_dev: false,
            compatibility_level: root_module.compatibility_level,
            yanked: false,
            yanked_reason: None,
            deprecated: false,
            deps: root_deps.clone(),
        });
        for key in root_deps {
            graph.add_edge(root_key.clone(), key);
        }
        for module in &modules {
            graph.add_module(module.clone());
            let key = ModuleKey::new(module.name.clone(), module.version.clone());
            for dependency in &module.deps {
                graph.add_edge(key.clone(), dependency.clone());
            }
        }
        Ok((modules, graph, warnings))
    }

    /// Find a non-yanked replacement for a yanked version.
    async fn find_non_yanked_version(
        &self,
        module_name: &str,
        requested_version: &str,
    ) -> Result<Option<String>> {
        let metadata = self.fetch_metadata(module_name).await?;

        if !metadata.yanked_versions.contains_key(requested_version) {
            return Ok(Some(requested_version.to_string()));
        }

        Ok(metadata
            .versions
            .iter()
            .filter(|version| !metadata.yanked_versions.contains_key(*version))
            .filter(|version| compare_versions(version, requested_version) >= 0)
            .min_by(|a, b| compare_versions(a, b).cmp(&0))
            .cloned())
    }

    async fn fetch_metadata(&self, module: &str) -> Result<crate::ModuleMetadata> {
        tokio::time::timeout(
            Duration::from_secs(self.options.timeout_secs),
            self.registry.get_metadata(module),
        )
        .await
        .map_err(|_| Error::Resolution(format!("metadata fetch for {module} timed out")))?
    }
}

/// Resolve dependencies from MODULE.bazel content.
///
/// This is a convenience function for simple resolution use cases.
impl Resolver<crate::RegistryClient> {
    /// Resolve dependencies from MODULE.bazel content with default registry.
    pub async fn resolve(content: &str, options: ResolverOptions) -> Result<ResolutionResult> {
        let module_info = parse_module_content(content)?;
        let config = crate::RegistryClientConfig {
            request_timeout: Duration::from_secs(options.timeout_secs),
            ..crate::RegistryClientConfig::default()
        };
        let registries = options
            .registries
            .iter()
            .map(|url| {
                crate::RegistryClient::with_config(url, config.clone())
                    .map(|client| Arc::new(client) as Arc<dyn Registry>)
            })
            .collect::<Result<Vec<_>>>()?;
        if registries.is_empty() {
            return Err(Error::Resolution(
                "at least one registry is required".into(),
            ));
        }
        let resolver = Resolver::new(crate::RegistryChain::new(registries), options);
        resolver.resolve_dependencies(&module_info).await
    }

    /// Resolve dependencies from MODULE.bazel file.
    pub async fn resolve_file(
        path: &std::path::Path,
        options: ResolverOptions,
    ) -> Result<ResolutionResult> {
        let content = tokio::fs::read_to_string(path).await.map_err(Error::Io)?;
        Self::resolve(&content, options).await
    }
}

/// Use the same AST parser for root, override and registry module files.
fn parse_module_content(content: &str) -> Result<ModuleInfo> {
    Ok(crate::ModuleFile::parse(content)?.info)
}

/// Dev dependencies in transitive modules are always ignored by Bazel.
fn dependencies(
    module: &ModuleInfo,
    include_dev: bool,
) -> impl Iterator<Item = (&Dependency, bool)> {
    module
        .deps
        .iter()
        .map(|dep| (dep, dep.dev_dependency))
        .chain(module.dev_deps.iter().map(|dep| (dep, true)))
        .filter(move |(_, dev)| !dev || include_dev)
}

#[cfg(test)]
mod tests;
