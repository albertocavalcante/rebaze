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

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use tokio::sync::{Mutex, RwLock, Semaphore};
use tracing::{debug, info, trace, warn};

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
    /// Compatibility level (if known).
    compatibility_level: Option<u32>,
}

/// Context for graph building.
struct GraphBuildContext {
    /// Dependency graph: module name -> version -> request info.
    dep_graph: HashMap<String, HashMap<String, DepRequest>>,
    /// Module dependencies: module name -> list of dependency names.
    module_deps: HashMap<String, Vec<String>>,
    /// Visiting set for cycle detection: module@version -> true.
    visiting: HashSet<String>,
    /// Overrides indexed by module name.
    overrides: HashMap<String, Override>,
    /// Pre-parsed MODULE.bazel for overridden modules.
    override_modules: HashMap<String, ModuleInfo>,
    /// Cache of fetched modules: `module@version` -> `ModuleInfo`.
    cache: HashMap<String, ModuleInfo>,
}

impl GraphBuildContext {
    fn new(overrides: Vec<Override>, override_modules: HashMap<String, ModuleInfo>) -> Self {
        let indexed_overrides = overrides
            .into_iter()
            .filter_map(|o| {
                let name = match &o {
                    Override::SingleVersion { module, .. } => module.as_str().to_string(),
                    Override::Git { module, .. } => module.as_str().to_string(),
                    Override::LocalPath { module, .. } => module.as_str().to_string(),
                    Override::Archive { module, .. } => module.as_str().to_string(),
                    Override::MultipleVersion { module, .. } => module.as_str().to_string(),
                };
                if name.is_empty() {
                    None
                } else {
                    Some((name, o))
                }
            })
            .collect();

        Self {
            dep_graph: HashMap::new(),
            module_deps: HashMap::new(),
            visiting: HashSet::new(),
            overrides: indexed_overrides,
            override_modules,
            cache: HashMap::new(),
        }
    }
}

/// Task for the worker pool.
#[derive(Debug, Clone)]
struct DepTask {
    name: String,
    version: String,
    path: Vec<String>,
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
        let context = Arc::new(Mutex::new(GraphBuildContext::new(
            root_module.overrides.clone(),
            override_modules,
        )));

        // Build the dependency graph
        let stats = Arc::new(Mutex::new(ResolutionStats::default()));

        self.build_dependency_graph(root_module, context.clone(), stats.clone())
            .await?;

        let mut ctx = context.lock().await;
        let mut stats = stats.lock().await;

        // Substitute yanked versions if enabled
        if self.options.substitute_yanked {
            self.substitute_yanked_versions(&mut ctx).await;
        }

        // Apply overrides
        self.apply_overrides(&mut ctx.dep_graph, &root_module.overrides);

        // Apply MVS: select highest version for each module
        let selected_versions = self.apply_mvs(&ctx.dep_graph);

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
            .build_resolution_result(
                &selected_versions,
                &ctx.module_deps,
                root_module,
                &mut stats,
            )
            .await?;

        warnings.extend(more_warnings);

        // Update stats
        stats.total_modules = modules.len();
        stats.direct_deps = root_module.deps.len();
        if self.options.include_dev_deps {
            stats.direct_deps += root_module.dev_deps.len();
        }
        stats.transitive_deps = stats.total_modules.saturating_sub(stats.direct_deps);

        Ok(ResolutionResult {
            root: root_module.clone(),
            modules,
            graph,
            stats: stats.clone(),
            warnings,
        })
    }

    /// Build the dependency graph by recursively fetching modules.
    async fn build_dependency_graph(
        &self,
        root_module: &ModuleInfo,
        context: Arc<Mutex<GraphBuildContext>>,
        stats: Arc<Mutex<ResolutionStats>>,
    ) -> Result<()> {
        // Create a channel for tasks
        let (tx, rx) = async_channel::bounded::<DepTask>(self.options.concurrency * 2 + 100);

        // Semaphore for limiting concurrency
        let semaphore = Arc::new(Semaphore::new(self.options.concurrency));

        // Error holder
        let first_error: Arc<Mutex<Option<Error>>> = Arc::new(Mutex::new(None));

        // Process root module dependencies first
        {
            let mut ctx = context.lock().await;
            self.process_deps(root_module, &["<root>".to_string()], &mut ctx, &tx)
                .await?;
        }

        // Close sender after enqueueing root deps
        drop(tx);

        // Spawn workers
        let mut handles = Vec::new();
        for _ in 0..self.options.concurrency {
            let rx = rx.clone();
            let semaphore = semaphore.clone();
            let context = context.clone();
            let registry = self.registry.clone();
            let first_error = first_error.clone();
            let options = self.options.clone();
            let stats = stats.clone();

            let handle = tokio::spawn(async move {
                while let Ok(task) = rx.recv().await {
                    // Check if we should stop
                    if first_error.lock().await.is_some() {
                        continue;
                    }

                    let _permit = semaphore.acquire().await.unwrap();

                    // Check depth limit
                    if task.path.len() > options.max_depth {
                        let mut err = first_error.lock().await;
                        if err.is_none() {
                            *err = Some(Error::MaxDepthExceeded(options.max_depth));
                        }
                        continue;
                    }

                    // Check cache first
                    let cache_key = format!("{}@{}", task.name, task.version);
                    let cached = {
                        let ctx = context.lock().await;
                        ctx.cache.get(&cache_key).cloned()
                    };

                    let module_info = if let Some(info) = cached {
                        stats.lock().await.cache_hits += 1;
                        info
                    } else {
                        // Fetch from registry
                        match registry.get_module(&task.name, &task.version).await {
                            Ok(info) => {
                                stats.lock().await.registry_fetches += 1;
                                // Cache it
                                {
                                    let mut ctx = context.lock().await;
                                    ctx.cache.insert(cache_key, info.clone());
                                }
                                info
                            }
                            Err(e) => {
                                // Check if it's a "not found" error
                                if matches!(
                                    &e,
                                    Error::ModuleNotFound(_) | Error::VersionNotFound { .. }
                                ) {
                                    debug!(
                                        "Module {}@{} not found, removing from graph",
                                        task.name, task.version
                                    );
                                    // Remove from graph
                                    let mut ctx = context.lock().await;
                                    if let Some(versions) = ctx.dep_graph.get_mut(&task.name) {
                                        versions.remove(&task.version);
                                        if versions.is_empty() {
                                            ctx.dep_graph.remove(&task.name);
                                        }
                                    }
                                    continue;
                                }

                                let mut err = first_error.lock().await;
                                if err.is_none() {
                                    *err = Some(Error::Resolution(format!(
                                        "fetch module {}@{}: {}",
                                        task.name, task.version, e
                                    )));
                                }
                                continue;
                            }
                        }
                    };

                    // Process this module's dependencies
                    // We need to create a new sender for child tasks
                    // But since we closed the original sender, we need to handle this differently
                    // For now, process synchronously (the Go version uses a goroutine pattern)
                    let mut ctx = context.lock().await;

                    // Capture this module's dependencies
                    let mut deps = Vec::new();
                    for dep in &module_info.deps {
                        if !dep.dev_dependency || options.include_dev_deps {
                            deps.push(dep.name.as_str().to_string());
                        }
                    }
                    if options.include_dev_deps {
                        for dep in &module_info.dev_deps {
                            deps.push(dep.name.as_str().to_string());
                        }
                    }
                    if !deps.is_empty() && !module_info.name.as_str().is_empty() {
                        ctx.module_deps
                            .insert(module_info.name.as_str().to_string(), deps);
                    }

                    // Process each dependency
                    for dep in module_info.deps.iter().chain(module_info.dev_deps.iter()) {
                        let is_dev = dep.dev_dependency
                            || module_info.dev_deps.iter().any(|d| d.name == dep.name);
                        if is_dev && !options.include_dev_deps {
                            continue;
                        }

                        let dep_name = dep.name.as_str();
                        let mut effective_version = dep.version.clone();
                        let mut skip_fetch = false;

                        // Check for overrides
                        if let Some(ovr) = ctx.overrides.get(dep_name) {
                            match ovr {
                                Override::SingleVersion { version, .. } => {
                                    effective_version = version.clone();
                                }
                                Override::Git { .. }
                                | Override::LocalPath { .. }
                                | Override::Archive { .. } => {
                                    skip_fetch = true;
                                }
                                Override::MultipleVersion { .. } => {
                                    // MultipleVersion doesn't change the requested version
                                }
                            }
                        }

                        // Add to dep graph
                        let versions = ctx
                            .dep_graph
                            .entry(dep_name.to_string())
                            .or_insert_with(HashMap::new);

                        let version_str = effective_version.as_str();
                        if let Some(existing) = versions.get_mut(version_str) {
                            existing.required_by.push(task.path.last().unwrap().clone());
                            if is_dev {
                                existing.dev_dependency = true;
                            }
                        } else {
                            versions.insert(
                                version_str.to_string(),
                                DepRequest {
                                    version: effective_version.clone(),
                                    dev_dependency: is_dev,
                                    required_by: vec![task.path.last().unwrap().clone()],
                                    compatibility_level: None,
                                },
                            );
                        }

                        if skip_fetch {
                            // Check if we have override module info
                            // Clone the deps to avoid borrow issues
                            let override_deps =
                                ctx.override_modules.get(dep_name).map(|m| m.deps.clone());

                            if let Some(deps) = override_deps {
                                let dep_key =
                                    format!("{}@{}", dep_name, effective_version.as_str());
                                let mut dep_path = task.path.clone();
                                dep_path.push(dep_key.clone());

                                // Check depth
                                if dep_path.len() > options.max_depth {
                                    continue;
                                }

                                // Check if already visited
                                if !ctx.visiting.insert(dep_key) {
                                    continue;
                                }

                                // Process override module's deps recursively
                                // (simplified - in production this would be more sophisticated)
                                for sub_dep in &deps {
                                    let sub_versions = ctx
                                        .dep_graph
                                        .entry(sub_dep.name.as_str().to_string())
                                        .or_insert_with(HashMap::new);

                                    let sub_ver = sub_dep.version.as_str();
                                    if !sub_versions.contains_key(sub_ver) {
                                        sub_versions.insert(
                                            sub_ver.to_string(),
                                            DepRequest {
                                                version: sub_dep.version.clone(),
                                                dev_dependency: sub_dep.dev_dependency,
                                                required_by: vec![dep_name.to_string()],
                                                compatibility_level: None,
                                            },
                                        );
                                    }
                                }
                            }
                            continue;
                        }

                        // Check if already being processed
                        let dep_key = format!("{}@{}", dep_name, effective_version.as_str());
                        if ctx.visiting.contains(&dep_key) {
                            trace!("Skipping already visiting: {}", dep_key);
                            continue;
                        }
                        ctx.visiting.insert(dep_key.clone());

                        // Note: We can't easily send more tasks from here since we closed the sender.
                        // In a full implementation, we'd need a different approach (like recursive spawning
                        // or keeping the sender alive with a counter). For now, transitive deps are
                        // handled through the cache and subsequent resolution passes.
                    }
                }
            });

            handles.push(handle);
        }

        // Wait for all workers
        for handle in handles {
            let _ = handle.await;
        }

        // Check for errors
        let err = first_error.lock().await.take();
        if let Some(e) = err {
            return Err(e);
        }

        // Do additional passes to resolve transitive dependencies
        // This is needed because the worker pool pattern above doesn't easily support
        // spawning new tasks from within workers after the sender is dropped.
        self.resolve_transitive_deps(context.clone(), stats).await?;

        Ok(())
    }

    /// Resolve transitive dependencies that weren't fetched in the first pass.
    async fn resolve_transitive_deps(
        &self,
        context: Arc<Mutex<GraphBuildContext>>,
        stats: Arc<Mutex<ResolutionStats>>,
    ) -> Result<()> {
        let max_iterations = 100; // Prevent infinite loops
        for iteration in 0..max_iterations {
            let pending: Vec<(String, String)> = {
                let ctx = context.lock().await;
                let mut pending = Vec::new();
                for (name, versions) in &ctx.dep_graph {
                    for version in versions.keys() {
                        let cache_key = format!("{name}@{version}");
                        if !ctx.cache.contains_key(&cache_key) && !ctx.visiting.contains(&cache_key)
                        {
                            // Check if this is an overridden module that shouldn't be fetched
                            let skip = if let Some(ovr) = ctx.overrides.get(name) {
                                matches!(
                                    ovr,
                                    Override::Git { .. }
                                        | Override::LocalPath { .. }
                                        | Override::Archive { .. }
                                )
                            } else {
                                false
                            };
                            if !skip {
                                pending.push((name.clone(), version.clone()));
                            }
                        }
                    }
                }
                pending
            };

            if pending.is_empty() {
                debug!(
                    "Transitive dependency resolution complete after {} iterations",
                    iteration
                );
                break;
            }

            debug!(
                "Iteration {}: {} pending transitive dependencies",
                iteration,
                pending.len()
            );

            // Fetch pending modules concurrently
            let semaphore = Arc::new(Semaphore::new(self.options.concurrency));
            let mut handles = Vec::new();

            for (name, version) in pending {
                let semaphore = semaphore.clone();
                let registry = self.registry.clone();
                let context = context.clone();
                let options = self.options.clone();
                let stats = stats.clone();

                let handle = tokio::spawn(async move {
                    let _permit = semaphore.acquire().await.unwrap();
                    let cache_key = format!("{name}@{version}");

                    // Mark as visiting
                    {
                        let mut ctx = context.lock().await;
                        ctx.visiting.insert(cache_key.clone());
                    }

                    match registry.get_module(&name, &version).await {
                        Ok(info) => {
                            stats.lock().await.registry_fetches += 1;

                            let mut ctx = context.lock().await;
                            ctx.cache.insert(cache_key, info.clone());

                            // Add this module's deps to the graph
                            let mut deps = Vec::new();
                            for dep in info.deps.iter().chain(info.dev_deps.iter()) {
                                let is_dev = dep.dev_dependency
                                    || info.dev_deps.iter().any(|d| d.name == dep.name);
                                if is_dev && !options.include_dev_deps {
                                    continue;
                                }

                                deps.push(dep.name.as_str().to_string());

                                let dep_name = dep.name.as_str();
                                let mut effective_version = dep.version.clone();

                                // Check for overrides
                                if let Some(Override::SingleVersion {
                                    version: ovr_version,
                                    ..
                                }) = ctx.overrides.get(dep_name)
                                {
                                    effective_version = ovr_version.clone();
                                }

                                let versions = ctx
                                    .dep_graph
                                    .entry(dep_name.to_string())
                                    .or_insert_with(HashMap::new);

                                let version_str = effective_version.as_str();
                                if let Some(existing) = versions.get_mut(version_str) {
                                    existing.required_by.push(name.clone());
                                } else {
                                    versions.insert(
                                        version_str.to_string(),
                                        DepRequest {
                                            version: effective_version,
                                            dev_dependency: is_dev,
                                            required_by: vec![name.clone()],
                                            compatibility_level: None,
                                        },
                                    );
                                }
                            }

                            if !deps.is_empty() && !info.name.as_str().is_empty() {
                                ctx.module_deps.insert(info.name.as_str().to_string(), deps);
                            }
                        }
                        Err(e) => {
                            if matches!(
                                &e,
                                Error::ModuleNotFound(_) | Error::VersionNotFound { .. }
                            ) {
                                debug!(
                                    "Module {}@{} not found, removing from graph",
                                    name, version
                                );
                                let mut ctx = context.lock().await;
                                if let Some(versions) = ctx.dep_graph.get_mut(&name) {
                                    versions.remove(&version);
                                    if versions.is_empty() {
                                        ctx.dep_graph.remove(&name);
                                    }
                                }
                            } else {
                                warn!("Failed to fetch {}@{}: {}", name, version, e);
                            }
                        }
                    }
                });

                handles.push(handle);
            }

            // Wait for all fetches
            for handle in handles {
                let _ = handle.await;
            }
        }

        Ok(())
    }

    /// Process dependencies of a module.
    async fn process_deps(
        &self,
        module: &ModuleInfo,
        path: &[String],
        ctx: &mut GraphBuildContext,
        tx: &async_channel::Sender<DepTask>,
    ) -> Result<()> {
        // Capture this module's dependencies for graph building
        let mut deps = Vec::new();
        for dep in &module.deps {
            if !dep.dev_dependency || self.options.include_dev_deps {
                deps.push(dep.name.as_str().to_string());
            }
        }
        if self.options.include_dev_deps {
            for dep in &module.dev_deps {
                deps.push(dep.name.as_str().to_string());
            }
        }
        if !deps.is_empty() && !module.name.as_str().is_empty() {
            ctx.module_deps
                .insert(module.name.as_str().to_string(), deps);
        }

        // Process each dependency
        let all_deps: Vec<&Dependency> = module.deps.iter().chain(module.dev_deps.iter()).collect();

        for dep in all_deps {
            let is_dev = dep.dev_dependency || module.dev_deps.iter().any(|d| d.name == dep.name);
            if is_dev && !self.options.include_dev_deps {
                continue;
            }

            let dep_name = dep.name.as_str();
            let mut effective_version = dep.version.clone();
            let mut skip_fetch = false;

            // Check for overrides
            if let Some(ovr) = ctx.overrides.get(dep_name) {
                match ovr {
                    Override::SingleVersion { version, .. } => {
                        effective_version = version.clone();
                    }
                    Override::Git { .. }
                    | Override::LocalPath { .. }
                    | Override::Archive { .. } => {
                        skip_fetch = true;
                    }
                    Override::MultipleVersion { .. } => {
                        // MultipleVersion doesn't change the requested version
                    }
                }
            }

            // Add to dep graph
            let versions = ctx.dep_graph.entry(dep_name.to_string()).or_default();

            let version_str = effective_version.as_str();
            let parent = path.last().map_or("<root>", String::as_str);

            if let Some(existing) = versions.get_mut(version_str) {
                existing.required_by.push(parent.to_string());
                if is_dev {
                    existing.dev_dependency = true;
                }
            } else {
                versions.insert(
                    version_str.to_string(),
                    DepRequest {
                        version: effective_version.clone(),
                        dev_dependency: is_dev,
                        required_by: vec![parent.to_string()],
                        compatibility_level: None,
                    },
                );
            }

            if skip_fetch {
                // Handle override modules
                if let Some(override_module) = ctx.override_modules.get(dep_name).cloned() {
                    let dep_key = format!("{}@{}", dep_name, effective_version.as_str());
                    let mut dep_path = path.to_vec();
                    dep_path.push(dep_key.clone());

                    // Check depth
                    if dep_path.len() > self.options.max_depth {
                        return Err(Error::MaxDepthExceeded(self.options.max_depth));
                    }

                    // Check if already visited
                    if !ctx.visiting.insert(dep_key) {
                        continue;
                    }

                    // Process override module's deps recursively (with Box::pin for recursion)
                    Box::pin(self.process_deps(&override_module, &dep_path, ctx, tx)).await?;
                }
                continue;
            }

            // Check if already being processed
            let dep_key = format!("{}@{}", dep_name, effective_version.as_str());
            if ctx.visiting.contains(&dep_key) {
                trace!("Skipping already visiting: {}", dep_key);
                continue;
            }
            ctx.visiting.insert(dep_key.clone());

            // Check depth limit
            let mut dep_path = path.to_vec();
            dep_path.push(dep_key);
            if dep_path.len() > self.options.max_depth {
                return Err(Error::MaxDepthExceeded(self.options.max_depth));
            }

            // Enqueue task for worker
            let task = DepTask {
                name: dep_name.to_string(),
                version: version_str.to_string(),
                path: dep_path,
            };

            if tx.send(task).await.is_err() {
                // Channel closed, this is fine
                break;
            }
        }

        Ok(())
    }

    /// Apply overrides to the dependency graph.
    fn apply_overrides(
        &self,
        dep_graph: &mut HashMap<String, HashMap<String, DepRequest>>,
        overrides: &[Override],
    ) {
        for ovr in overrides {
            match ovr {
                Override::SingleVersion {
                    module, version, ..
                } => {
                    let module_name = module.as_str();
                    let version_str = version.as_str();

                    if let Some(versions) = dep_graph.get(module_name) {
                        let mut new_versions = HashMap::new();
                        if let Some(req) = versions.get(version_str) {
                            new_versions.insert(version_str.to_string(), req.clone());
                        } else {
                            new_versions.insert(
                                version_str.to_string(),
                                DepRequest {
                                    version: version.clone(),
                                    dev_dependency: false,
                                    required_by: vec!["<override>".to_string()],
                                    compatibility_level: None,
                                },
                            );
                        }
                        dep_graph.insert(module_name.to_string(), new_versions);
                    } else {
                        // Create entry for nonexistent module
                        let mut new_versions = HashMap::new();
                        new_versions.insert(
                            version_str.to_string(),
                            DepRequest {
                                version: version.clone(),
                                dev_dependency: false,
                                required_by: vec!["<override>".to_string()],
                                compatibility_level: None,
                            },
                        );
                        dep_graph.insert(module_name.to_string(), new_versions);
                    }
                }
                Override::Git { .. }
                | Override::LocalPath { .. }
                | Override::Archive { .. }
                | Override::MultipleVersion { .. } => {
                    // These overrides don't modify the version selection
                }
            }
        }
    }

    /// Apply MVS: select the highest version for each module.
    fn apply_mvs(
        &self,
        dep_graph: &HashMap<String, HashMap<String, DepRequest>>,
    ) -> HashMap<String, DepRequest> {
        let mut selected = HashMap::new();

        for (module_name, versions) in dep_graph {
            let mut max_req: Option<&DepRequest> = None;

            for req in versions.values() {
                if max_req.is_none()
                    || compare_versions(req.version.as_str(), max_req.unwrap().version.as_str()) > 0
                {
                    max_req = Some(req);
                }
            }

            if let Some(req) = max_req {
                selected.insert(module_name.clone(), req.clone());
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
        module_deps: &HashMap<String, Vec<String>>,
        root_module: &ModuleInfo,
        stats: &mut ResolutionStats,
    ) -> Result<(Vec<ResolvedModule>, DependencyGraph, Vec<String>)> {
        let mut modules = Vec::new();
        let mut graph = DependencyGraph::new();
        let mut warnings = Vec::new();

        // Build set of direct dependency names
        let direct_deps: HashSet<String> = root_module
            .deps
            .iter()
            .chain(root_module.dev_deps.iter())
            .map(|d| d.name.as_str().to_string())
            .collect();

        // Build set of selected module names for filtering dependencies
        let selected_names: HashSet<String> = selected_versions.keys().cloned().collect();

        for (module_name, req) in selected_versions {
            let is_direct = direct_deps.contains(module_name);

            // Get filtered dependencies
            let deps: Vec<ModuleKey> = module_deps
                .get(module_name)
                .map(|deps| {
                    deps.iter()
                        .filter(|dep| selected_names.contains(*dep))
                        .filter_map(|dep| {
                            selected_versions.get(dep).map(|r| {
                                ModuleKey::new(
                                    ModuleName::new(dep.clone()).unwrap(),
                                    r.version.clone(),
                                )
                            })
                        })
                        .collect()
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
                compatibility_level: req.compatibility_level.unwrap_or(0),
                yanked: false,
                yanked_reason: None,
                deprecated: false,
                deps: deps.clone(),
            };

            // Add to graph
            let key = ModuleKey::new(resolved.name.clone(), resolved.version.clone());
            for dep_key in &deps {
                graph.add_edge(key.clone(), dep_key.clone());
            }
            graph.add_module(resolved.clone());

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
                match self.registry.get_metadata(module.name.as_str()).await {
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
                    Err(e) => {
                        debug!(
                            "Failed to fetch metadata for {}: {}",
                            module.name.as_str(),
                            e
                        );
                    }
                }
            }
        }

        Ok((modules, graph, warnings))
    }

    /// Substitute yanked versions with non-yanked alternatives.
    async fn substitute_yanked_versions(&self, ctx: &mut GraphBuildContext) {
        let module_names: Vec<String> = ctx.dep_graph.keys().cloned().collect();

        for module_name in module_names {
            let versions: Vec<String> = ctx
                .dep_graph
                .get(&module_name)
                .map(|v| v.keys().cloned().collect())
                .unwrap_or_default();

            let mut replacements = Vec::new();

            for version in versions {
                if let Some(replacement) =
                    self.find_non_yanked_version(&module_name, &version).await
                    && replacement != version
                {
                    replacements.push((version, replacement));
                }
            }

            // Apply replacements
            if let Some(versions) = ctx.dep_graph.get_mut(&module_name) {
                for (old_ver, new_ver) in replacements {
                    if let Some(mut req) = versions.remove(&old_ver) {
                        req.version = Version::new(new_ver.clone()).unwrap();
                        versions.insert(new_ver, req);
                    }
                }
            }
        }
    }

    /// Find a non-yanked replacement for a yanked version.
    async fn find_non_yanked_version(
        &self,
        module_name: &str,
        requested_version: &str,
    ) -> Option<String> {
        let metadata = self.registry.get_metadata(module_name).await.ok()?;

        if !metadata.yanked_versions.contains_key(requested_version) {
            return Some(requested_version.to_string());
        }

        // Find non-yanked versions
        let non_yanked: Vec<&String> = metadata
            .versions
            .iter()
            .filter(|v| !metadata.yanked_versions.contains_key(*v))
            .collect();

        // Find the lowest non-yanked version >= requested
        for candidate in non_yanked {
            if compare_versions(candidate, requested_version) >= 0 {
                return Some(candidate.clone());
            }
        }

        // No suitable replacement found
        Some(requested_version.to_string())
    }
}

/// Resolve dependencies from MODULE.bazel content.
///
/// This is a convenience function for simple resolution use cases.
impl Resolver<crate::RegistryClient> {
    /// Resolve dependencies from MODULE.bazel content with default registry.
    pub async fn resolve(content: &str, options: ResolverOptions) -> Result<ResolutionResult> {
        let module_info = parse_module_content(content)?;
        let registry = crate::RegistryClient::bcr();
        let resolver = Self::new(registry, options);
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

/// Parse MODULE.bazel content into `ModuleInfo`.
///
/// This is a simplified parser. A full implementation would use the parser module.
fn parse_module_content(content: &str) -> Result<ModuleInfo> {
    // Use the parser module if available, otherwise use a basic regex parser
    // For now, implement a basic parser that handles common cases
    use regex::Regex;

    let module_re =
        Regex::new(r#"module\s*\(\s*name\s*=\s*"([^"]+)"\s*,\s*version\s*=\s*"([^"]+)""#)
            .map_err(|e| Error::Parse(e.to_string()))?;

    let bazel_dep_re = Regex::new(
        r#"bazel_dep\s*\(\s*name\s*=\s*"([^"]+)"\s*,\s*version\s*=\s*"([^"]+)"(?:\s*,\s*dev_dependency\s*=\s*(True|False))?\s*\)"#,
    )
    .map_err(|e| Error::Parse(e.to_string()))?;

    let (name, version) = if let Some(caps) = module_re.captures(content) {
        (
            caps.get(1)
                .map_or_else(String::new, |m| m.as_str().to_string()),
            caps.get(2)
                .map_or_else(String::new, |m| m.as_str().to_string()),
        )
    } else {
        (String::new(), String::new())
    };

    let mut deps = Vec::new();
    let mut dev_deps = Vec::new();

    for caps in bazel_dep_re.captures_iter(content) {
        let dep_name = caps.get(1).map_or("", |m| m.as_str());
        let dep_version = caps.get(2).map_or("", |m| m.as_str());
        let is_dev = caps.get(3).is_some_and(|m| m.as_str() == "True");

        let dep = Dependency {
            name: ModuleName::new(dep_name)?,
            version: Version::new(dep_version)?,
            max_version: None,
            repo_name: None,
            dev_dependency: is_dev,
        };

        if is_dev {
            dev_deps.push(dep);
        } else {
            deps.push(dep);
        }
    }

    Ok(ModuleInfo {
        name: ModuleName::new(name)?,
        version: Version::new(if version.is_empty() {
            "0.0.0"
        } else {
            &version
        })?,
        compatibility_level: 0,
        bazel_compatibility: Vec::new(),
        deps,
        dev_deps,
        overrides: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version_comparison() {
        assert!(compare_versions("1.0.0", "0.9.0") > 0);
        assert!(compare_versions("1.0.0", "1.0.0") == 0);
        assert!(compare_versions("1.0.0", "1.0.1") < 0);
        assert!(compare_versions("2.0.0", "1.9.9") > 0);
        assert!(compare_versions("1.0.0-alpha", "1.0.0") < 0);
        assert!(compare_versions("1.0.0-alpha", "1.0.0-beta") < 0);
    }

    #[test]
    fn test_parse_module_content() {
        let content = r#"
            module(name = "my_project", version = "1.0.0")
            bazel_dep(name = "rules_rust", version = "0.40.0")
            bazel_dep(name = "rules_go", version = "0.50.0", dev_dependency = True)
        "#;

        let info = parse_module_content(content).unwrap();
        assert_eq!(info.name.as_str(), "my_project");
        assert_eq!(info.version.as_str(), "1.0.0");
        assert_eq!(info.deps.len(), 1);
        assert_eq!(info.deps[0].name.as_str(), "rules_rust");
        assert_eq!(info.dev_deps.len(), 1);
        assert_eq!(info.dev_deps[0].name.as_str(), "rules_go");
    }

    #[test]
    fn test_resolver_options_default() {
        let options = ResolverOptions::default();
        assert_eq!(options.concurrency, DEFAULT_MAX_CONCURRENCY);
        assert_eq!(options.max_depth, DEFAULT_MAX_DEPTH);
        assert!(!options.include_dev_deps);
        assert!(options.check_yanked);
    }
}
