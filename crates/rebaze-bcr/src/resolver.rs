//! Version resolution logic for BCR dependencies.
//!
//! This module handles resolving package lists to BCR modules,
//! detecting version conflicts, and generating fallback information.

use crate::catalog::BcrCatalog;
use crate::mapping::{
    get_boost_module, is_boost_component, lookup_cmake_package, lookup_gradle_package,
};
use std::collections::BTreeMap;

/// A successfully resolved BCR dependency.
#[derive(Debug, Clone)]
pub struct ResolvedDep {
    /// The original package name from CMake/Gradle
    pub original_name: String,

    /// BCR module name
    pub module: String,

    /// Selected version (latest by default)
    pub version: String,

    /// Bazel target label (if different from @module)
    pub target: Option<String>,
}

/// A package that could not be resolved to BCR.
#[derive(Debug, Clone)]
pub struct UnresolvedDep {
    /// The original package name
    pub name: String,

    /// Reason for not being resolved
    pub reason: String,
}

/// A version conflict between dependencies.
#[derive(Debug, Clone)]
pub struct VersionConflict {
    /// The BCR module with conflicting versions
    pub module: String,

    /// The conflicting version requests
    pub requested_versions: Vec<String>,

    /// The selected version (if any)
    pub selected: Option<String>,
}

/// Result of resolving a set of dependencies.
#[derive(Debug, Clone, Default)]
pub struct ResolutionResult {
    /// Successfully resolved dependencies
    pub resolved: Vec<ResolvedDep>,

    /// Dependencies that need fallback resolution
    pub unresolved: Vec<UnresolvedDep>,

    /// Version conflicts detected
    pub conflicts: Vec<VersionConflict>,
}

impl ResolutionResult {
    /// Check if all dependencies were resolved.
    #[must_use]
    pub const fn is_complete(&self) -> bool {
        self.unresolved.is_empty() && self.conflicts.is_empty()
    }

    /// Get the number of resolved dependencies.
    #[must_use]
    pub const fn resolved_count(&self) -> usize {
        self.resolved.len()
    }

    /// Get the number of unresolved dependencies.
    #[must_use]
    pub const fn unresolved_count(&self) -> usize {
        self.unresolved.len()
    }
}

/// Resolve CMake packages to BCR dependencies.
pub fn resolve_packages(catalog: &BcrCatalog, packages: &[String]) -> ResolutionResult {
    let mut result = ResolutionResult::default();
    let mut seen_modules: BTreeMap<String, String> = BTreeMap::new();

    for package in packages {
        // Handle Boost components specially
        if let Some(dep) = try_resolve_boost_component(catalog, package) {
            add_resolved_dep(&mut result, &mut seen_modules, dep);
            continue;
        }

        // Look up the package in cmake mappings
        if let Some(mapping) = lookup_cmake_package(catalog, package) {
            // Get version from the module info
            let version = catalog
                .get_latest_version(&mapping.module)
                .map(str::to_string)
                .unwrap_or_default();

            let dep = ResolvedDep {
                original_name: package.clone(),
                module: mapping.module.clone(),
                version,
                target: mapping.target.clone(),
            };

            add_resolved_dep(&mut result, &mut seen_modules, dep);
        } else {
            // Package not found in BCR
            result.unresolved.push(UnresolvedDep {
                name: package.clone(),
                reason: "Not found in BCR catalog".to_string(),
            });
        }
    }

    result
}

/// Resolve Gradle dependencies to BCR dependencies.
pub fn resolve_gradle_packages(catalog: &BcrCatalog, coordinates: &[String]) -> ResolutionResult {
    let mut result = ResolutionResult::default();
    let mut seen_modules: BTreeMap<String, String> = BTreeMap::new();

    for coordinate in coordinates {
        if let Some(mapping) = lookup_gradle_package(catalog, coordinate) {
            let version = catalog
                .get_latest_version(&mapping.module)
                .map(str::to_string)
                .unwrap_or_default();

            let dep = ResolvedDep {
                original_name: coordinate.clone(),
                module: mapping.module.clone(),
                version,
                target: mapping.target.clone(),
            };

            add_resolved_dep(&mut result, &mut seen_modules, dep);
        } else {
            result.unresolved.push(UnresolvedDep {
                name: coordinate.clone(),
                reason: "Not found in BCR catalog (Gradle mapping)".to_string(),
            });
        }
    }

    result
}

/// Try to resolve a Boost component to its BCR module.
fn try_resolve_boost_component(catalog: &BcrCatalog, package: &str) -> Option<ResolvedDep> {
    // Check if this looks like a Boost component reference
    // Patterns: "Boost::filesystem", "boost_filesystem", "boost.filesystem"
    let component = extract_boost_component(package)?;

    if !is_boost_component(catalog, &component) {
        return None;
    }

    let module_name = get_boost_module(&component);
    let version = catalog
        .get_latest_version(&module_name)
        .map(str::to_string)
        .unwrap_or_default();

    Some(ResolvedDep {
        original_name: package.to_string(),
        module: module_name.clone(),
        version,
        target: Some(format!("@{module_name}")),
    })
}

/// Extract a Boost component name from various formats.
fn extract_boost_component(package: &str) -> Option<String> {
    // Handle "Boost::component" format
    if package.starts_with("Boost::") || package.starts_with("boost::") {
        return Some(package.split("::").nth(1)?.to_lowercase());
    }

    // Handle "boost_component" format
    if let Some(suffix) = package.strip_prefix("boost_") {
        return Some(suffix.to_lowercase());
    }
    if let Some(suffix) = package.strip_prefix("Boost_") {
        return Some(suffix.to_lowercase());
    }

    // Handle "boost.component" format
    if let Some(suffix) = package.strip_prefix("boost.") {
        return Some(suffix.to_lowercase());
    }

    None
}

/// Add a resolved dependency, checking for duplicates.
fn add_resolved_dep(
    result: &mut ResolutionResult,
    seen: &mut BTreeMap<String, String>,
    dep: ResolvedDep,
) {
    if let Some(existing_version) = seen.get(&dep.module) {
        // Check for version conflict
        if *existing_version != dep.version && !dep.version.is_empty() {
            // Record conflict but don't add duplicate
            let conflict = VersionConflict {
                module: dep.module.clone(),
                requested_versions: vec![existing_version.clone(), dep.version],
                selected: Some(existing_version.clone()),
            };
            result.conflicts.push(conflict);
        }
        // Skip duplicate module
        return;
    }

    seen.insert(dep.module.clone(), dep.version.clone());
    result.resolved.push(dep);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embedded::load_embedded_catalog;

    #[test]
    fn test_resolve_known_packages() {
        let catalog = load_embedded_catalog();
        let packages = vec!["protobuf".to_string(), "fmt".to_string()];

        let result = resolve_packages(&catalog, &packages);

        assert_eq!(result.resolved.len(), 2);
        assert!(result.unresolved.is_empty());
        assert!(result.is_complete());
    }

    #[test]
    fn test_resolve_unknown_packages() {
        let catalog = load_embedded_catalog();
        let packages = vec!["unknown_lib_xyz".to_string()];

        let result = resolve_packages(&catalog, &packages);

        assert!(result.resolved.is_empty());
        assert_eq!(result.unresolved.len(), 1);
        assert!(!result.is_complete());
    }

    #[test]
    fn test_resolve_mixed_packages() {
        let catalog = load_embedded_catalog();
        let packages = vec![
            "protobuf".to_string(),
            "unknown_lib".to_string(),
            "fmt".to_string(),
        ];

        let result = resolve_packages(&catalog, &packages);

        assert_eq!(result.resolved.len(), 2);
        assert_eq!(result.unresolved.len(), 1);
    }

    #[test]
    fn test_resolve_boost_component() {
        let catalog = load_embedded_catalog();
        let packages = vec!["Boost::filesystem".to_string()];

        let result = resolve_packages(&catalog, &packages);

        assert_eq!(result.resolved.len(), 1);
        assert_eq!(result.resolved[0].module, "boost.filesystem");
    }

    #[test]
    fn test_resolve_boost_underscore_format() {
        let catalog = load_embedded_catalog();
        let packages = vec!["boost_system".to_string()];

        let result = resolve_packages(&catalog, &packages);

        assert_eq!(result.resolved.len(), 1);
        assert_eq!(result.resolved[0].module, "boost.system");
    }

    #[test]
    fn test_extract_boost_component() {
        assert_eq!(
            extract_boost_component("Boost::filesystem"),
            Some("filesystem".to_string())
        );
        assert_eq!(
            extract_boost_component("boost::system"),
            Some("system".to_string())
        );
        assert_eq!(
            extract_boost_component("boost_asio"),
            Some("asio".to_string())
        );
        assert_eq!(
            extract_boost_component("boost.log"),
            Some("log".to_string())
        );
        assert_eq!(extract_boost_component("protobuf"), None);
    }

    #[test]
    fn test_deduplicate_modules() {
        let catalog = load_embedded_catalog();
        let packages = vec![
            "protobuf".to_string(),
            "Protobuf".to_string(),
            "google-protobuf".to_string(),
        ];

        let result = resolve_packages(&catalog, &packages);

        // Should only have one protobuf entry
        assert_eq!(result.resolved.len(), 1);
        assert_eq!(result.resolved[0].module, "protobuf");
    }

    #[test]
    fn test_resolve_gradle_packages() {
        let catalog = load_embedded_catalog();
        let coords = vec!["com.google.protobuf:protobuf-java:3.21.0".to_string()];

        let result = resolve_gradle_packages(&catalog, &coords);

        assert_eq!(result.resolved.len(), 1);
        assert_eq!(result.resolved[0].module, "protobuf");
    }
}
