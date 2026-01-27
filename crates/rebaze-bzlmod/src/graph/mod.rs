//! Dependency graph and queries.
//!
//! This module provides a dependency graph implementation with query capabilities
//! for exploring module relationships, explaining version selections, and exporting
//! to various formats (DOT, JSON).

// Allow missing const fn for methods that could technically be const but
// aren't to preserve API flexibility for future changes.
#![allow(clippy::missing_const_for_fn)]

use crate::{ModuleKey, ResolvedModule, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};

/// Dependency graph with query capabilities.
///
/// Supports bidirectional traversal (dependencies and dependents)
/// and provides query methods for explaining version selections.
#[derive(Debug, Clone, Default)]
pub struct DependencyGraph {
    /// Root module of the graph.
    root: Option<ModuleKey>,
    /// Modules in the graph.
    modules: BTreeMap<ModuleKey, ResolvedModule>,
    /// Edges: from -> [to]
    edges: BTreeMap<ModuleKey, Vec<ModuleKey>>,
    /// Reverse edges: to -> [from]
    reverse_edges: BTreeMap<ModuleKey, Vec<ModuleKey>>,
    /// Tracks requested versions: `module_name` -> `version` -> `[requesters]`
    requested_versions: HashMap<String, HashMap<String, Vec<ModuleKey>>>,
    /// Overrides: `module_name` -> `overridden_version`
    overrides: HashMap<String, String>,
}

impl DependencyGraph {
    /// Create a new empty graph.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a new graph with a root module.
    #[must_use]
    pub fn with_root(root: ModuleKey) -> Self {
        Self {
            root: Some(root),
            ..Self::default()
        }
    }

    /// Set the root module of the graph.
    pub fn set_root(&mut self, root: ModuleKey) {
        self.root = Some(root);
    }

    /// Get the root module key.
    #[must_use]
    pub fn root(&self) -> Option<&ModuleKey> {
        self.root.as_ref()
    }

    /// Add a module to the graph.
    pub fn add_module(&mut self, module: ResolvedModule) {
        let key = ModuleKey::new(module.name.clone(), module.version.clone());
        self.modules.insert(key, module);
    }

    /// Add an edge (dependency relationship).
    pub fn add_edge(&mut self, from: ModuleKey, to: ModuleKey) {
        self.edges.entry(from.clone()).or_default().push(to.clone());
        self.reverse_edges.entry(to).or_default().push(from);
    }

    /// Record a version request for later explanation.
    /// Call this during dependency graph construction, before MVS selection.
    pub fn record_request(&mut self, module_name: &str, version: &str, requester: ModuleKey) {
        self.requested_versions
            .entry(module_name.to_string())
            .or_default()
            .entry(version.to_string())
            .or_default()
            .push(requester);
    }

    /// Record that a module has a version override.
    pub fn record_override(&mut self, module_name: &str, version: &str) {
        self.overrides
            .insert(module_name.to_string(), version.to_string());
    }

    /// Get a module by its key.
    #[must_use]
    pub fn get(&self, key: &ModuleKey) -> Option<&ResolvedModule> {
        self.modules.get(key)
    }

    /// Get a module by name (any version).
    #[must_use]
    pub fn get_by_name(&self, name: &str) -> Option<(&ModuleKey, &ResolvedModule)> {
        self.modules
            .iter()
            .find(|(key, _)| key.name.as_str() == name)
    }

    /// Check if the graph contains a module.
    #[must_use]
    pub fn contains(&self, key: &ModuleKey) -> bool {
        self.modules.contains_key(key)
    }

    /// Check if the graph contains a module by name.
    #[must_use]
    pub fn contains_name(&self, name: &str) -> bool {
        self.get_by_name(name).is_some()
    }

    /// Get the number of modules in the graph.
    #[must_use]
    pub fn len(&self) -> usize {
        self.modules.len()
    }

    /// Check if the graph is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.modules.is_empty()
    }

    /// Get all modules in the graph.
    #[must_use]
    pub fn modules(&self) -> &BTreeMap<ModuleKey, ResolvedModule> {
        &self.modules
    }

    /// Get direct dependencies of a module.
    #[must_use]
    pub fn direct_deps(&self, key: &ModuleKey) -> Vec<&ResolvedModule> {
        self.edges
            .get(key)
            .map(|deps| deps.iter().filter_map(|k| self.modules.get(k)).collect())
            .unwrap_or_default()
    }

    /// Get direct dependency keys of a module.
    #[must_use]
    pub fn direct_dep_keys(&self, key: &ModuleKey) -> Vec<&ModuleKey> {
        self.edges
            .get(key)
            .map(|deps| deps.iter().collect())
            .unwrap_or_default()
    }

    /// Get modules that directly depend on the given module (reverse dependencies).
    #[must_use]
    pub fn reverse_deps(&self, key: &ModuleKey) -> Vec<&ResolvedModule> {
        self.reverse_edges
            .get(key)
            .map(|deps| deps.iter().filter_map(|k| self.modules.get(k)).collect())
            .unwrap_or_default()
    }

    /// Get keys of modules that directly depend on the given module.
    #[must_use]
    pub fn reverse_dep_keys(&self, key: &ModuleKey) -> Vec<&ModuleKey> {
        self.reverse_edges
            .get(key)
            .map(|deps| deps.iter().collect())
            .unwrap_or_default()
    }

    /// Get all transitive dependencies of a module (BFS order).
    #[must_use]
    pub fn transitive_deps(&self, key: &ModuleKey) -> Vec<ModuleKey> {
        let mut result = Vec::new();
        let mut visited = BTreeSet::new();
        let mut queue = VecDeque::new();

        visited.insert(key.clone());
        queue.push_back(key.clone());

        while let Some(current) = queue.pop_front() {
            if let Some(deps) = self.edges.get(&current) {
                for dep in deps {
                    if !visited.contains(dep) {
                        visited.insert(dep.clone());
                        result.push(dep.clone());
                        queue.push_back(dep.clone());
                    }
                }
            }
        }

        result
    }

    /// Get all transitive dependents of a module (BFS order).
    #[must_use]
    pub fn transitive_dependents(&self, key: &ModuleKey) -> Vec<ModuleKey> {
        let mut result = Vec::new();
        let mut visited = BTreeSet::new();
        let mut queue = VecDeque::new();

        visited.insert(key.clone());
        queue.push_back(key.clone());

        while let Some(current) = queue.pop_front() {
            if let Some(deps) = self.reverse_edges.get(&current) {
                for dep in deps {
                    if !visited.contains(dep) {
                        visited.insert(dep.clone());
                        result.push(dep.clone());
                        queue.push_back(dep.clone());
                    }
                }
            }
        }

        result
    }

    /// Find the shortest path between two modules using BFS.
    ///
    /// Returns `None` if no path exists.
    #[must_use]
    pub fn path(&self, from: &ModuleKey, to: &ModuleKey) -> Option<Vec<ModuleKey>> {
        if from == to {
            return Some(vec![from.clone()]);
        }

        // BFS to find shortest path
        let mut visited = BTreeSet::new();
        let mut queue: VecDeque<Vec<ModuleKey>> = VecDeque::new();

        visited.insert(from.clone());
        queue.push_back(vec![from.clone()]);

        while let Some(current_path) = queue.pop_front() {
            let current = current_path.last()?;

            if let Some(deps) = self.edges.get(current) {
                for dep in deps {
                    if dep == to {
                        let mut result = current_path;
                        result.push(dep.clone());
                        return Some(result);
                    }

                    if !visited.contains(dep) {
                        visited.insert(dep.clone());
                        let mut new_path = current_path.clone();
                        new_path.push(dep.clone());
                        queue.push_back(new_path);
                    }
                }
            }
        }

        None
    }

    /// Find all paths between two modules using DFS.
    ///
    /// This can be expensive for large graphs with many paths.
    #[must_use]
    pub fn all_paths(&self, from: &ModuleKey, to: &ModuleKey) -> Vec<Vec<ModuleKey>> {
        let mut result = Vec::new();
        let mut visited = BTreeSet::new();
        let path = vec![from.clone()];

        self.find_all_paths_dfs(from, to, path, &mut visited, &mut result);

        result
    }

    /// DFS helper for finding all paths.
    fn find_all_paths_dfs(
        &self,
        current: &ModuleKey,
        target: &ModuleKey,
        path: Vec<ModuleKey>,
        visited: &mut BTreeSet<ModuleKey>,
        result: &mut Vec<Vec<ModuleKey>>,
    ) {
        if current == target {
            result.push(path);
            return;
        }

        visited.insert(current.clone());

        if let Some(deps) = self.edges.get(current) {
            for dep in deps {
                if !visited.contains(dep) {
                    let mut new_path = path.clone();
                    new_path.push(dep.clone());
                    self.find_all_paths_dfs(dep, target, new_path, visited, result);
                }
            }
        }

        visited.remove(current);
    }

    /// Explain why a module has a specific version.
    ///
    /// Returns an explanation including all dependency chains that lead to this module
    /// and why this particular version was selected.
    ///
    /// # Errors
    ///
    /// Returns an error if the module is not found in the graph.
    pub fn explain(&self, module_name: &str) -> Result<Explanation> {
        let (key, _module) = self
            .get_by_name(module_name)
            .ok_or_else(|| crate::Error::ModuleNotFound(module_name.to_string()))?;

        let key = key.clone();
        let mut explanation = Explanation {
            module: module_name.to_string(),
            version: key.version.to_string(),
            requesters: Vec::new(),
            reason: String::new(),
            selection: None,
            dependency_chains: Vec::new(),
        };

        // Find all paths from root to this module
        if let Some(root) = &self.root {
            let paths = self.all_paths(root, &key);
            for path in paths {
                let mut chain = DependencyChain {
                    path: path.clone(),
                    requested_version: String::new(),
                };

                // Get the requested version from the immediate parent
                if path.len() >= 2 {
                    let parent = &path[path.len() - 2];
                    if let Some(versions) = self.requested_versions.get(module_name) {
                        for (version, requesters) in versions {
                            if requesters.iter().any(|r| r == parent) {
                                version.clone_into(&mut chain.requested_version);
                                break;
                            }
                        }
                    }
                }

                explanation.dependency_chains.push(chain);
            }
        }

        let version_str = key.version.as_str();

        // Build selection info
        explanation.selection = Some(self.build_selection_info(module_name, version_str));

        // Build requesters info
        if let Some(versions) = self.requested_versions.get(module_name) {
            for (version, requesters) in versions {
                for requester in requesters {
                    let requester_info = RequesterInfo {
                        requester: requester.clone(),
                        requested_version: version.clone(),
                        path: self
                            .root
                            .as_ref()
                            .and_then(|root| self.path(root, requester))
                            .unwrap_or_default(),
                    };
                    explanation.requesters.push(requester_info);
                }
            }
        }

        // Build reason string
        explanation.reason = self.build_reason_string(module_name, version_str);

        Ok(explanation)
    }

    /// Build selection info for a module.
    fn build_selection_info(&self, module_name: &str, selected_version: &str) -> SelectionInfo {
        let mut info = SelectionInfo {
            strategy: SelectionStrategy::Mvs,
            selected_version: selected_version.to_string(),
            candidates: Vec::new(),
            deciding_factor: String::new(),
        };

        // Check if this was an override
        if let Some(override_version) = self.overrides.get(module_name)
            && override_version == selected_version
        {
            info.strategy = SelectionStrategy::Override;
            info.deciding_factor = "single_version_override".to_string();
            return info;
        }

        // Get all version candidates
        if let Some(versions) = self.requested_versions.get(module_name) {
            for (version, requesters) in versions {
                let selected = version == selected_version;
                let candidate = VersionCandidate {
                    version: version.clone(),
                    requested_by: requesters.clone(),
                    selected,
                    rejection_reason: if selected {
                        None
                    } else {
                        Some("lower version (MVS selects highest)".to_string())
                    },
                };
                info.candidates.push(candidate);
            }
        }

        // Determine strategy
        if info.candidates.len() <= 1 {
            info.deciding_factor = "only version requested".to_string();
        } else {
            info.deciding_factor = "highest version among candidates".to_string();
        }

        info
    }

    /// Build a human-readable reason string for version selection.
    fn build_reason_string(&self, module_name: &str, selected_version: &str) -> String {
        if let Some(override_version) = self.overrides.get(module_name)
            && override_version == selected_version
        {
            return format!(
                "{module_name}@{selected_version} was selected due to single_version_override"
            );
        }

        if let Some(versions) = self.requested_versions.get(module_name) {
            if versions.len() == 1 {
                return format!("{module_name}@{selected_version} was the only version requested");
            }
            return format!(
                "{module_name}@{selected_version} was selected by MVS (highest among {} candidates)",
                versions.len()
            );
        }

        format!("{module_name}@{selected_version}")
    }

    /// Get all modules that requested a specific module (with their requested versions).
    #[must_use]
    pub fn why_included(&self, module_name: &str) -> Option<Vec<DependencyChain>> {
        let (key, _) = self.get_by_name(module_name)?;
        let key = key.clone();

        let root = self.root.as_ref()?;
        let paths = self.all_paths(root, &key);

        Some(
            paths
                .into_iter()
                .map(|path| DependencyChain {
                    path,
                    requested_version: String::new(),
                })
                .collect(),
        )
    }

    /// Export graph as DOT format for visualization.
    #[must_use]
    pub fn to_dot(&self) -> String {
        use std::fmt::Write;

        let mut buf = String::new();

        buf.push_str("digraph dependencies {\n");
        buf.push_str("  rankdir=LR;\n");
        buf.push_str("  node [shape=box];\n\n");

        // Add nodes
        for (key, module) in &self.modules {
            let label = format!("{}\\n{}", key.name, key.version);
            let mut attrs = format!("label=\"{label}\"");

            if Some(key) == self.root.as_ref() {
                attrs.push_str(", style=bold");
            }
            if module.is_dev {
                attrs.push_str(", style=dashed");
            }

            let _ = writeln!(buf, "  \"{key}\" [{attrs}];");
        }

        buf.push('\n');

        // Add edges
        for (from, deps) in &self.edges {
            for to in deps {
                let _ = writeln!(buf, "  \"{from}\" -> \"{to}\";");
            }
        }

        buf.push_str("}\n");
        buf
    }

    /// Export graph as JSON (Bazel-compatible format).
    ///
    /// # Errors
    ///
    /// Returns an error if JSON serialization fails.
    pub fn to_json(&self) -> Result<String> {
        let bazel_graph = self.to_bazel_format();
        serde_json::to_string_pretty(&bazel_graph).map_err(Into::into)
    }

    /// Convert graph to Bazel's JSON format.
    fn to_bazel_format(&self) -> BazelModGraph {
        let Some(root) = &self.root else {
            return BazelModGraph::default();
        };

        if !self.modules.contains_key(root) {
            return BazelModGraph::default();
        }

        let mut visited = BTreeSet::new();
        let cycles = self.find_cycles();
        let cycle_keys: BTreeSet<_> = cycles.into_iter().flatten().collect();

        BazelModGraph {
            key: root.to_string(),
            name: Some(root.name.to_string()),
            version: Some(root.version.to_string()),
            root: true,
            dependencies: self.build_bazel_deps(root, &mut visited, &cycle_keys),
            indirect_dependencies: Vec::new(),
            cycles: Vec::new(),
        }
    }

    /// Recursively build Bazel-format dependencies.
    fn build_bazel_deps(
        &self,
        key: &ModuleKey,
        visited: &mut BTreeSet<ModuleKey>,
        cycle_keys: &BTreeSet<ModuleKey>,
    ) -> Vec<BazelDependency> {
        let Some(deps) = self.edges.get(key) else {
            return Vec::new();
        };

        let mut result = Vec::with_capacity(deps.len());

        for dep_key in deps {
            if visited.contains(dep_key) {
                // Already visited, mark as unexpanded to avoid infinite recursion
                result.push(BazelDependency {
                    key: dep_key.to_string(),
                    unexpanded: true,
                    ..Default::default()
                });
                continue;
            }

            visited.insert(dep_key.clone());

            let mut bazel_dep = BazelDependency {
                key: dep_key.to_string(),
                ..Default::default()
            };

            if cycle_keys.contains(dep_key) {
                // This node is part of a cycle
                bazel_dep.cycles = vec![BazelDependency {
                    key: dep_key.to_string(),
                    ..Default::default()
                }];
            } else {
                bazel_dep.dependencies = self.build_bazel_deps(dep_key, visited, cycle_keys);
            }

            result.push(bazel_dep);
        }

        result
    }

    /// Check if the graph has cycles.
    #[must_use]
    pub fn has_cycles(&self) -> bool {
        let mut visited = BTreeSet::new();
        let mut rec_stack = BTreeSet::new();

        for key in self.modules.keys() {
            if self.has_cycle_dfs(key, &mut visited, &mut rec_stack) {
                return true;
            }
        }

        false
    }

    /// DFS helper for cycle detection.
    fn has_cycle_dfs(
        &self,
        key: &ModuleKey,
        visited: &mut BTreeSet<ModuleKey>,
        rec_stack: &mut BTreeSet<ModuleKey>,
    ) -> bool {
        if !visited.contains(key) {
            visited.insert(key.clone());
            rec_stack.insert(key.clone());

            if let Some(deps) = self.edges.get(key) {
                for dep in deps {
                    if !visited.contains(dep) {
                        if self.has_cycle_dfs(dep, visited, rec_stack) {
                            return true;
                        }
                    } else if rec_stack.contains(dep) {
                        return true;
                    }
                }
            }
        }

        rec_stack.remove(key);
        false
    }

    /// Find all cycles in the graph.
    #[must_use]
    pub fn find_cycles(&self) -> Vec<Vec<ModuleKey>> {
        let mut cycles = Vec::new();
        let mut visited = BTreeSet::new();
        let mut rec_stack = BTreeSet::new();
        let mut path = Vec::new();

        for key in self.modules.keys() {
            if !visited.contains(key) {
                self.find_cycles_dfs(key, &mut visited, &mut rec_stack, &mut path, &mut cycles);
            }
        }

        cycles
    }

    /// DFS helper for finding cycles.
    fn find_cycles_dfs(
        &self,
        key: &ModuleKey,
        visited: &mut BTreeSet<ModuleKey>,
        rec_stack: &mut BTreeSet<ModuleKey>,
        path: &mut Vec<ModuleKey>,
        cycles: &mut Vec<Vec<ModuleKey>>,
    ) {
        visited.insert(key.clone());
        rec_stack.insert(key.clone());
        path.push(key.clone());

        if let Some(deps) = self.edges.get(key) {
            for dep in deps {
                if !visited.contains(dep) {
                    self.find_cycles_dfs(dep, visited, rec_stack, path, cycles);
                } else if rec_stack.contains(dep) {
                    // Found a cycle, extract it
                    if let Some(cycle_start) = path.iter().position(|k| k == dep) {
                        let cycle: Vec<_> = path[cycle_start..].to_vec();
                        cycles.push(cycle);
                    }
                }
            }
        }

        path.pop();
        rec_stack.remove(key);
    }

    /// Get all root nodes (nodes with no dependents).
    #[must_use]
    pub fn roots(&self) -> Vec<&ModuleKey> {
        self.modules
            .keys()
            .filter(|key| self.reverse_edges.get(*key).is_none_or(Vec::is_empty))
            .collect()
    }

    /// Get all leaf nodes (nodes with no dependencies).
    #[must_use]
    pub fn leaves(&self) -> Vec<&ModuleKey> {
        self.modules
            .keys()
            .filter(|key| self.edges.get(*key).is_none_or(Vec::is_empty))
            .collect()
    }

    /// Get statistics about the graph.
    #[must_use]
    pub fn stats(&self) -> GraphStats {
        let total_modules = self.modules.len();

        let direct_dependencies = self
            .root
            .as_ref()
            .and_then(|root| self.edges.get(root))
            .map_or(0, Vec::len);

        let transitive_dependencies = total_modules.saturating_sub(direct_dependencies + 1);

        let dev_dependencies = self.modules.values().filter(|m| m.is_dev).count();

        let max_depth = self.calculate_max_depth();

        GraphStats {
            total_modules,
            direct_dependencies,
            transitive_dependencies,
            max_depth,
            dev_dependencies,
        }
    }

    /// Calculate the maximum depth of the dependency tree.
    fn calculate_max_depth(&self) -> usize {
        let Some(root) = &self.root else {
            return 0;
        };

        let mut depths: HashMap<ModuleKey, usize> = HashMap::new();
        let mut max_depth = 0;

        self.calculate_depth_dfs(root, 0, &mut depths, &mut max_depth);

        max_depth
    }

    /// DFS helper for depth calculation.
    fn calculate_depth_dfs(
        &self,
        key: &ModuleKey,
        depth: usize,
        depths: &mut HashMap<ModuleKey, usize>,
        max_depth: &mut usize,
    ) {
        if let Some(&existing) = depths.get(key)
            && existing >= depth
        {
            return;
        }

        depths.insert(key.clone(), depth);
        if depth > *max_depth {
            *max_depth = depth;
        }

        if let Some(deps) = self.edges.get(key) {
            for dep in deps {
                self.calculate_depth_dfs(dep, depth + 1, depths, max_depth);
            }
        }
    }

    /// Export as a text representation.
    #[must_use]
    pub fn to_text(&self) -> String {
        use std::fmt::Write;

        let mut buf = String::new();
        let stats = self.stats();

        if let Some(root) = &self.root {
            let _ = writeln!(buf, "Dependency Graph (root: {root})");
        } else {
            buf.push_str("Dependency Graph\n");
        }
        buf.push_str(&"=".repeat(60));
        buf.push_str("\n\n");

        let _ = writeln!(buf, "Total modules: {}", stats.total_modules);
        let _ = writeln!(buf, "Direct dependencies: {}", stats.direct_dependencies);
        let _ = writeln!(
            buf,
            "Transitive dependencies: {}",
            stats.transitive_dependencies
        );
        let _ = writeln!(buf, "Max depth: {}", stats.max_depth);
        if stats.dev_dependencies > 0 {
            let _ = writeln!(buf, "Dev dependencies: {}", stats.dev_dependencies);
        }
        buf.push('\n');

        buf.push_str("Dependency Tree:\n");

        if let Some(root) = &self.root {
            let mut visited = BTreeSet::new();
            self.print_tree(&mut buf, root, "", true, &mut visited);
        }

        buf
    }

    /// Helper for printing tree structure.
    fn print_tree(
        &self,
        buf: &mut String,
        key: &ModuleKey,
        prefix: &str,
        is_last: bool,
        visited: &mut BTreeSet<ModuleKey>,
    ) {
        let connector = if is_last { "└── " } else { "├── " };

        if !prefix.is_empty() {
            buf.push_str(prefix);
            buf.push_str(connector);
        }
        buf.push_str(&key.to_string());

        if let Some(module) = self.modules.get(key) {
            if module.is_dev {
                buf.push_str(" (dev)");
            }
        }

        if visited.contains(key) {
            buf.push_str(" (circular)\n");
            return;
        }
        buf.push('\n');

        visited.insert(key.clone());

        if let Some(deps) = self.edges.get(key) {
            let child_prefix = if prefix.is_empty() {
                String::new()
            } else if is_last {
                format!("{prefix}    ")
            } else {
                format!("{prefix}│   ")
            };

            for (i, dep) in deps.iter().enumerate() {
                let is_last_child = i == deps.len() - 1;
                self.print_tree(buf, dep, &child_prefix, is_last_child, visited);
            }
        }

        visited.remove(key);
    }

    /// Export as a flat module list (excluding root).
    #[must_use]
    pub fn to_module_list(&self) -> Vec<ModuleListEntry> {
        let mut modules: Vec<ModuleListEntry> = self
            .modules
            .iter()
            .filter(|(key, _)| Some(*key) != self.root.as_ref())
            .map(|(key, module)| {
                let required_by: Vec<String> = self
                    .reverse_edges
                    .get(key)
                    .map(|deps| deps.iter().map(ToString::to_string).collect())
                    .unwrap_or_default();

                ModuleListEntry {
                    name: key.name.to_string(),
                    version: key.version.to_string(),
                    dev_dependency: module.is_dev,
                    required_by,
                }
            })
            .collect();

        modules.sort_by(|a, b| a.name.cmp(&b.name));
        modules
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::label::{ModuleName, Version};

    fn create_test_module(name: &str, version: &str, is_dev: bool) -> ResolvedModule {
        ResolvedModule {
            name: ModuleName::new(name).unwrap(),
            version: Version::new(version).unwrap(),
            registry: "https://bcr.bazel.build".to_string(),
            is_direct: false,
            is_dev,
            compatibility_level: 0,
            yanked: false,
            yanked_reason: None,
            deprecated: false,
            deps: Vec::new(),
        }
    }

    fn create_test_graph() -> DependencyGraph {
        // Create graph:
        //   root@1.0.0
        //   ├── a@1.0.0
        //   │   └── c@2.0.0
        //   └── b@1.0.0
        //       └── c@2.0.0 (shared)
        let root = ModuleKey::new(
            ModuleName::new("root").unwrap(),
            Version::new("1.0.0").unwrap(),
        );
        let a = ModuleKey::new(
            ModuleName::new("a").unwrap(),
            Version::new("1.0.0").unwrap(),
        );
        let b = ModuleKey::new(
            ModuleName::new("b").unwrap(),
            Version::new("1.0.0").unwrap(),
        );
        let c = ModuleKey::new(
            ModuleName::new("c").unwrap(),
            Version::new("2.0.0").unwrap(),
        );

        let mut graph = DependencyGraph::with_root(root.clone());

        graph.add_module(create_test_module("root", "1.0.0", false));
        graph.add_module(create_test_module("a", "1.0.0", false));
        graph.add_module(create_test_module("b", "1.0.0", false));
        graph.add_module(create_test_module("c", "2.0.0", false));

        graph.add_edge(root.clone(), a.clone());
        graph.add_edge(root, b.clone());
        graph.add_edge(a, c.clone());
        graph.add_edge(b, c);

        graph
    }

    #[test]
    fn test_new_graph() {
        let graph = DependencyGraph::new();
        assert!(graph.is_empty());
        assert!(graph.root().is_none());
    }

    #[test]
    fn test_with_root() {
        let root = ModuleKey::new(
            ModuleName::new("test").unwrap(),
            Version::new("1.0.0").unwrap(),
        );
        let graph = DependencyGraph::with_root(root.clone());
        assert_eq!(graph.root(), Some(&root));
    }

    #[test]
    fn test_add_module() {
        let mut graph = DependencyGraph::new();
        let module = create_test_module("test", "1.0.0", false);
        graph.add_module(module);
        assert_eq!(graph.len(), 1);
    }

    #[test]
    fn test_direct_deps() {
        let graph = create_test_graph();
        let root = graph.root().unwrap();
        let deps = graph.direct_deps(root);
        assert_eq!(deps.len(), 2);
    }

    #[test]
    fn test_reverse_deps() {
        let graph = create_test_graph();
        let c = ModuleKey::new(
            ModuleName::new("c").unwrap(),
            Version::new("2.0.0").unwrap(),
        );
        let rev_deps = graph.reverse_deps(&c);
        assert_eq!(rev_deps.len(), 2); // a and b depend on c
    }

    #[test]
    fn test_transitive_deps() {
        let graph = create_test_graph();
        let root = graph.root().unwrap();
        let deps = graph.transitive_deps(root);
        assert_eq!(deps.len(), 3); // a, b, c
    }

    #[test]
    fn test_path_same_node() {
        let graph = create_test_graph();
        let root = graph.root().unwrap();
        let path = graph.path(root, root);
        assert_eq!(path, Some(vec![root.clone()]));
    }

    #[test]
    fn test_path_exists() {
        let graph = create_test_graph();
        let root = graph.root().unwrap();
        let c = ModuleKey::new(
            ModuleName::new("c").unwrap(),
            Version::new("2.0.0").unwrap(),
        );
        let path = graph.path(root, &c);
        assert!(path.is_some());
        let path = path.unwrap();
        assert_eq!(path.len(), 3); // root -> a/b -> c
        assert_eq!(path.first(), Some(root));
        assert_eq!(path.last(), Some(&c));
    }

    #[test]
    fn test_path_not_exists() {
        let graph = create_test_graph();
        let root = graph.root().unwrap();
        let nonexistent = ModuleKey::new(
            ModuleName::new("x").unwrap(),
            Version::new("1.0.0").unwrap(),
        );
        let path = graph.path(root, &nonexistent);
        assert!(path.is_none());
    }

    #[test]
    fn test_all_paths() {
        let graph = create_test_graph();
        let root = graph.root().unwrap();
        let c = ModuleKey::new(
            ModuleName::new("c").unwrap(),
            Version::new("2.0.0").unwrap(),
        );
        let paths = graph.all_paths(root, &c);
        assert_eq!(paths.len(), 2); // root->a->c and root->b->c
    }

    #[test]
    fn test_to_dot() {
        let graph = create_test_graph();
        let dot = graph.to_dot();
        assert!(dot.contains("digraph dependencies"));
        assert!(dot.contains("rankdir=LR"));
        assert!(dot.contains("->"));
    }

    #[test]
    fn test_to_json() {
        let graph = create_test_graph();
        let json = graph.to_json().unwrap();
        assert!(json.contains("root@1.0.0"));
        assert!(json.contains("dependencies"));
    }

    #[test]
    fn test_stats() {
        let graph = create_test_graph();
        let stats = graph.stats();
        assert_eq!(stats.total_modules, 4);
        assert_eq!(stats.direct_dependencies, 2);
        assert_eq!(stats.transitive_dependencies, 1); // only c is transitive
        assert_eq!(stats.max_depth, 2);
    }

    #[test]
    fn test_has_cycles_no_cycles() {
        let graph = create_test_graph();
        assert!(!graph.has_cycles());
    }

    #[test]
    fn test_has_cycles_with_cycles() {
        let a = ModuleKey::new(
            ModuleName::new("a").unwrap(),
            Version::new("1.0.0").unwrap(),
        );
        let b = ModuleKey::new(
            ModuleName::new("b").unwrap(),
            Version::new("1.0.0").unwrap(),
        );

        let mut graph = DependencyGraph::with_root(a.clone());
        graph.add_module(create_test_module("a", "1.0.0", false));
        graph.add_module(create_test_module("b", "1.0.0", false));
        graph.add_edge(a.clone(), b.clone());
        graph.add_edge(b, a);

        assert!(graph.has_cycles());
    }

    #[test]
    fn test_roots() {
        let graph = create_test_graph();
        let roots = graph.roots();
        assert_eq!(roots.len(), 1);
    }

    #[test]
    fn test_leaves() {
        let graph = create_test_graph();
        let leaves = graph.leaves();
        assert_eq!(leaves.len(), 1);
        assert_eq!(leaves[0].name.as_str(), "c");
    }

    #[test]
    fn test_explain() {
        let graph = create_test_graph();
        let explanation = graph.explain("c").unwrap();
        assert_eq!(explanation.module, "c");
        assert_eq!(explanation.version, "2.0.0");
        assert_eq!(explanation.dependency_chains.len(), 2);
    }

    #[test]
    fn test_explain_not_found() {
        let graph = create_test_graph();
        let result = graph.explain("nonexistent");
        assert!(result.is_err());
    }

    #[test]
    fn test_to_text() {
        let graph = create_test_graph();
        let text = graph.to_text();
        assert!(text.contains("Dependency Graph"));
        assert!(text.contains("root@1.0.0"));
        assert!(text.contains("Total modules: 4"));
    }

    #[test]
    fn test_to_module_list() {
        let graph = create_test_graph();
        let modules = graph.to_module_list();
        assert_eq!(modules.len(), 3); // excluding root
        assert_eq!(modules[0].name, "a");
        assert_eq!(modules[1].name, "b");
        assert_eq!(modules[2].name, "c");
    }

    #[test]
    fn test_dependency_chain_display() {
        let chain = DependencyChain {
            path: vec![
                ModuleKey::new(
                    ModuleName::new("root").unwrap(),
                    Version::new("1.0.0").unwrap(),
                ),
                ModuleKey::new(
                    ModuleName::new("a").unwrap(),
                    Version::new("1.0.0").unwrap(),
                ),
            ],
            requested_version: "1.5.0".to_string(),
        };
        let display = chain.to_string();
        assert!(display.contains("root@1.0.0"));
        assert!(display.contains("a@1.0.0"));
        assert!(display.contains("requested 1.5.0"));
    }

    #[test]
    fn test_record_request_and_override() {
        let root = ModuleKey::new(
            ModuleName::new("root").unwrap(),
            Version::new("1.0.0").unwrap(),
        );
        let mut graph = DependencyGraph::with_root(root.clone());
        graph.add_module(create_test_module("root", "1.0.0", false));
        graph.add_module(create_test_module("foo", "2.0.0", false));

        graph.record_request("foo", "1.0.0", root.clone());
        graph.record_request("foo", "2.0.0", root);
        graph.record_override("foo", "2.0.0");

        let foo = ModuleKey::new(
            ModuleName::new("foo").unwrap(),
            Version::new("2.0.0").unwrap(),
        );
        graph.add_edge(graph.root().unwrap().clone(), foo);

        let explanation = graph.explain("foo").unwrap();
        assert!(explanation.selection.is_some());
        let selection = explanation.selection.unwrap();
        assert_eq!(selection.strategy, SelectionStrategy::Override);
    }

    #[test]
    fn test_get_by_name() {
        let graph = create_test_graph();
        let (key, module) = graph.get_by_name("a").unwrap();
        assert_eq!(key.name.as_str(), "a");
        assert_eq!(module.name.as_str(), "a");
    }

    #[test]
    fn test_contains() {
        let graph = create_test_graph();
        let a = ModuleKey::new(
            ModuleName::new("a").unwrap(),
            Version::new("1.0.0").unwrap(),
        );
        assert!(graph.contains(&a));

        let x = ModuleKey::new(
            ModuleName::new("x").unwrap(),
            Version::new("1.0.0").unwrap(),
        );
        assert!(!graph.contains(&x));
    }

    #[test]
    fn test_contains_name() {
        let graph = create_test_graph();
        assert!(graph.contains_name("a"));
        assert!(!graph.contains_name("x"));
    }

    #[test]
    fn test_selection_strategy_display() {
        assert_eq!(SelectionStrategy::Mvs.to_string(), "mvs");
        assert_eq!(SelectionStrategy::Override.to_string(), "override");
        assert_eq!(
            SelectionStrategy::SingleVersionOverride.to_string(),
            "single_version_override"
        );
        assert_eq!(SelectionStrategy::Root.to_string(), "root");
    }

    #[test]
    fn test_empty_dependency_chain_display() {
        let chain = DependencyChain {
            path: Vec::new(),
            requested_version: String::new(),
        };
        assert_eq!(chain.to_string(), "");
    }

    #[test]
    fn test_transitive_dependents() {
        let graph = create_test_graph();
        let c = ModuleKey::new(
            ModuleName::new("c").unwrap(),
            Version::new("2.0.0").unwrap(),
        );
        let dependents = graph.transitive_dependents(&c);
        assert_eq!(dependents.len(), 3); // a, b, root
    }

    #[test]
    fn test_why_included() {
        let graph = create_test_graph();
        let chains = graph.why_included("c").unwrap();
        assert_eq!(chains.len(), 2);
    }

    #[test]
    fn test_why_included_not_found() {
        let graph = create_test_graph();
        let result = graph.why_included("nonexistent");
        assert!(result.is_none());
    }

    #[test]
    fn test_dev_dependency() {
        let root = ModuleKey::new(
            ModuleName::new("root").unwrap(),
            Version::new("1.0.0").unwrap(),
        );
        let dev = ModuleKey::new(
            ModuleName::new("dev").unwrap(),
            Version::new("1.0.0").unwrap(),
        );

        let mut graph = DependencyGraph::with_root(root.clone());
        graph.add_module(create_test_module("root", "1.0.0", false));
        graph.add_module(create_test_module("dev", "1.0.0", true));
        graph.add_edge(root, dev);

        let stats = graph.stats();
        assert_eq!(stats.dev_dependencies, 1);

        let text = graph.to_text();
        assert!(text.contains("Dev dependencies: 1"));
    }
}
