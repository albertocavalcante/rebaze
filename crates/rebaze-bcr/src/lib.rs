//! Bazel Central Registry (BCR) integration for rebaze.
//!
//! This crate provides functionality to map CMake and Gradle dependencies
//! to their BCR equivalents, enabling automatic `bazel_dep()` generation.
//!
//! # Example
//!
//! ```
//! use rebaze_bcr::{is_available, get_bcr_module, resolve_cmake_deps};
//!
//! // Check if a package is available in BCR
//! assert!(is_available("protobuf"));
//! assert!(is_available("Protobuf")); // Case-insensitive
//!
//! // Get the BCR module name for a CMake package
//! assert_eq!(get_bcr_module("google-protobuf"), Some("protobuf".to_string()));
//!
//! // Resolve multiple dependencies at once
//! let packages = vec!["protobuf".to_string(), "fmt".to_string()];
//! let result = resolve_cmake_deps(&packages);
//! assert!(!result.resolved.is_empty());
//! ```

mod catalog;
mod embedded;
mod mapping;
mod resolver;

pub use catalog::{BcrCatalog, BcrModule, BcrTarget, MappingConfidence, PackageMapping};
pub use resolver::{ResolutionResult, ResolvedDep, UnresolvedDep, VersionConflict};

use std::sync::OnceLock;

/// Global catalog instance, lazily initialized.
static CATALOG: OnceLock<BcrCatalog> = OnceLock::new();

/// Get the global BCR catalog instance.
fn catalog() -> &'static BcrCatalog {
    CATALOG.get_or_init(embedded::load_embedded_catalog)
}

/// Check if a CMake/Gradle package name is available in BCR.
///
/// This performs a case-insensitive lookup, handling common naming variations:
/// - `Protobuf`, `protobuf`, `PROTOBUF` -> all match
/// - `google-protobuf`, `google_protobuf` -> match via aliases
#[must_use]
pub fn is_available(package_name: &str) -> bool {
    mapping::lookup_cmake_package(catalog(), package_name).is_some()
}

/// Get the BCR module name for a CMake/Gradle package.
///
/// Returns `None` if the package is not found in the catalog.
#[must_use]
pub fn get_bcr_module(package_name: &str) -> Option<String> {
    mapping::lookup_cmake_package(catalog(), package_name).map(|m| m.module.clone())
}

/// Get the Bazel target label for a CMake package and optional component.
///
/// For packages without components, returns the default target.
/// For packages with components (like Boost), maps the component to its target.
///
/// # Example
///
/// ```
/// use rebaze_bcr::get_bazel_target;
///
/// // Simple package
/// assert_eq!(get_bazel_target("zlib", None), Some("@zlib".to_string()));
///
/// // Package with component
/// assert_eq!(
///     get_bazel_target("Boost", Some("filesystem")),
///     Some("@boost.filesystem".to_string())
/// );
/// ```
#[must_use]
pub fn get_bazel_target(package_name: &str, component: Option<&str>) -> Option<String> {
    let mapping = mapping::lookup_cmake_package(catalog(), package_name)?;

    component.map_or_else(
        // No component - return default target
        || {
            mapping.target.clone().or_else(|| {
                // If no explicit target, use @module_name format
                Some(format!("@{}", mapping.module))
            })
        },
        // Component specified - look up component-specific target
        |comp| {
            mapping
                .components
                .get(comp)
                .cloned()
                .or_else(|| mapping.target.clone())
        },
    )
}

/// Resolve a list of CMake package names to BCR dependencies.
///
/// Returns a `ResolutionResult` containing:
/// - `resolved`: Packages found in BCR with their module names and versions
/// - `unresolved`: Packages not found in BCR (need fallback strategy)
/// - `conflicts`: Version conflicts between dependencies (if any)
#[must_use]
pub fn resolve_cmake_deps(packages: &[String]) -> ResolutionResult {
    resolver::resolve_packages(catalog(), packages)
}

/// Resolve a list of Gradle dependency coordinates to BCR dependencies.
///
/// Gradle coordinates are in the format `group:artifact:version`.
#[must_use]
pub fn resolve_gradle_deps(coordinates: &[String]) -> ResolutionResult {
    resolver::resolve_gradle_packages(catalog(), coordinates)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_available() {
        // Known BCR packages
        assert!(is_available("protobuf"));
        assert!(is_available("Protobuf")); // Case insensitive
        assert!(is_available("fmt"));
        assert!(is_available("zlib"));

        // Unknown packages
        assert!(!is_available("some_unknown_package_xyz"));
    }

    #[test]
    fn test_get_bcr_module() {
        assert_eq!(get_bcr_module("protobuf"), Some("protobuf".to_string()));
        assert_eq!(get_bcr_module("Protobuf"), Some("protobuf".to_string()));
        assert_eq!(
            get_bcr_module("google-protobuf"),
            Some("protobuf".to_string())
        );
        assert_eq!(get_bcr_module("fmt"), Some("fmt".to_string()));
        assert_eq!(get_bcr_module("unknown_pkg"), None);
    }

    #[test]
    fn test_get_bazel_target() {
        // Simple package
        assert_eq!(get_bazel_target("zlib", None), Some("@zlib".to_string()));

        // Package with default target
        assert_eq!(
            get_bazel_target("protobuf", None),
            Some("@protobuf//:protobuf".to_string())
        );
    }

    #[test]
    fn test_resolve_cmake_deps() {
        let packages = vec![
            "protobuf".to_string(),
            "fmt".to_string(),
            "unknown_lib".to_string(),
        ];
        let result = resolve_cmake_deps(&packages);

        // protobuf and fmt should be resolved
        assert!(result.resolved.iter().any(|d| d.module == "protobuf"));
        assert!(result.resolved.iter().any(|d| d.module == "fmt"));

        // unknown_lib should be unresolved
        assert!(result.unresolved.iter().any(|u| u.name == "unknown_lib"));
    }
}
