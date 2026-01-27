//! Package name mapping from CMake/Gradle to BCR.
//!
//! This module handles the translation of package names from various
//! build systems to their BCR equivalents.

use crate::catalog::{BcrCatalog, PackageMapping};

/// Look up a CMake package name in the catalog.
///
/// This performs case-insensitive matching and handles common naming
/// variations (e.g., hyphens vs underscores).
pub fn lookup_cmake_package<'a>(
    catalog: &'a BcrCatalog,
    package_name: &str,
) -> Option<&'a PackageMapping> {
    // Try exact match first
    if let Some(mapping) = catalog.cmake_mappings.get(package_name) {
        return Some(mapping);
    }

    // Try case-insensitive match
    let lower = package_name.to_lowercase();
    for (name, mapping) in &catalog.cmake_mappings {
        if name.to_lowercase() == lower {
            return Some(mapping);
        }
    }

    // Try with hyphen/underscore normalization
    let normalized = normalize_package_name(package_name);
    for (name, mapping) in &catalog.cmake_mappings {
        if normalize_package_name(name) == normalized {
            return Some(mapping);
        }
    }

    None
}

/// Look up a Gradle artifact in the catalog.
///
/// Gradle coordinates are in the format `group:artifact:version`.
/// This function matches on `group:artifact` (ignoring version).
pub fn lookup_gradle_package<'a>(
    catalog: &'a BcrCatalog,
    coordinate: &str,
) -> Option<&'a PackageMapping> {
    // Extract group:artifact (remove version if present)
    let parts: Vec<&str> = coordinate.split(':').collect();
    let key = if parts.len() >= 2 {
        format!("{}:{}", parts[0], parts[1])
    } else {
        coordinate.to_string()
    };

    // Try exact match
    if let Some(mapping) = catalog.gradle_mappings.get(&key) {
        return Some(mapping);
    }

    // Try case-insensitive match
    let lower = key.to_lowercase();
    for (name, mapping) in &catalog.gradle_mappings {
        if name.to_lowercase() == lower {
            return Some(mapping);
        }
    }

    None
}

/// Normalize a package name for comparison.
///
/// - Converts to lowercase
/// - Replaces hyphens with underscores
/// - Removes common prefixes like "lib"
fn normalize_package_name(name: &str) -> String {
    let lower = name.to_lowercase();
    let normalized = lower.replace('-', "_");

    // Remove common "lib" prefix for matching
    normalized
        .strip_prefix("lib")
        .map_or_else(|| normalized.clone(), str::to_string)
}

/// Get the BCR module for a Boost component.
///
/// Boost is special because it's split into many modules in BCR,
/// one per component (e.g., boost.filesystem, boost.asio).
#[must_use]
pub fn get_boost_module(component: &str) -> String {
    format!("boost.{}", component.to_lowercase())
}

/// Check if a component name is a known Boost component.
#[must_use]
pub fn is_boost_component(catalog: &BcrCatalog, component: &str) -> bool {
    let module_name = get_boost_module(component);
    catalog.modules.contains_key(&module_name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embedded::load_embedded_catalog;

    #[test]
    fn test_lookup_cmake_exact() {
        let catalog = load_embedded_catalog();

        // Exact match
        let mapping = lookup_cmake_package(&catalog, "protobuf");
        assert!(mapping.is_some());
        assert_eq!(mapping.expect("has mapping").module, "protobuf");
    }

    #[test]
    fn test_lookup_cmake_case_insensitive() {
        let catalog = load_embedded_catalog();

        // Different case should still match
        let mapping = lookup_cmake_package(&catalog, "PROTOBUF");
        assert!(mapping.is_some());
        assert_eq!(mapping.expect("has mapping").module, "protobuf");
    }

    #[test]
    fn test_lookup_cmake_alias() {
        let catalog = load_embedded_catalog();

        // Aliases like google-protobuf should map to protobuf
        let mapping = lookup_cmake_package(&catalog, "google-protobuf");
        assert!(mapping.is_some());
        assert_eq!(mapping.expect("has mapping").module, "protobuf");
    }

    #[test]
    fn test_lookup_cmake_not_found() {
        let catalog = load_embedded_catalog();

        let mapping = lookup_cmake_package(&catalog, "nonexistent_package_xyz");
        assert!(mapping.is_none());
    }

    #[test]
    fn test_lookup_gradle_exact() {
        let catalog = load_embedded_catalog();

        let mapping = lookup_gradle_package(&catalog, "com.google.protobuf:protobuf-java:3.21.0");
        assert!(mapping.is_some());
        assert_eq!(mapping.expect("has mapping").module, "protobuf");
    }

    #[test]
    fn test_normalize_package_name() {
        assert_eq!(normalize_package_name("FlatBuffers"), "flatbuffers");
        assert_eq!(normalize_package_name("google-protobuf"), "google_protobuf");
        assert_eq!(normalize_package_name("libssh2"), "ssh2");
        assert_eq!(normalize_package_name("LibSSH2"), "ssh2");
    }

    #[test]
    fn test_get_boost_module() {
        assert_eq!(get_boost_module("filesystem"), "boost.filesystem");
        assert_eq!(get_boost_module("System"), "boost.system");
        assert_eq!(get_boost_module("ASIO"), "boost.asio");
    }

    #[test]
    fn test_is_boost_component() {
        let catalog = load_embedded_catalog();

        assert!(is_boost_component(&catalog, "filesystem"));
        assert!(is_boost_component(&catalog, "system"));
        assert!(!is_boost_component(&catalog, "nonexistent_component"));
    }
}
