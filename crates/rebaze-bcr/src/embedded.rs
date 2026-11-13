//! Embedded BCR catalog loading.
//!
//! This module handles loading the embedded catalog data from the
//! compiled-in JSON file.

use crate::catalog::BcrCatalog;

/// Embedded catalog JSON data.
const CATALOG_JSON: &str = include_str!("../data/bcr-catalog.json");

/// Load the embedded BCR catalog.
///
/// This parses the embedded JSON catalog at runtime. The catalog is
/// compiled into the binary for offline operation.
pub fn load_embedded_catalog() -> BcrCatalog {
    serde_json::from_str(CATALOG_JSON).unwrap_or_else(|e| {
        tracing::error!("Failed to parse embedded BCR catalog: {e}");
        BcrCatalog::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    /// Parse the embedded JSON without the runtime fallback to an empty catalog.
    fn strict_catalog() -> BcrCatalog {
        match serde_json::from_str(CATALOG_JSON) {
            Ok(catalog) => catalog,
            Err(error) => panic!("embedded BCR catalog must parse: {error}"),
        }
    }

    /// A label belongs to a module when it is `@module` or starts with `@module//`.
    fn label_belongs_to(label: &str, module: &str) -> bool {
        label
            .strip_prefix('@')
            .and_then(|rest| rest.strip_prefix(module))
            .is_some_and(|rest| rest.is_empty() || rest.starts_with("//"))
    }

    #[test]
    fn test_load_embedded_catalog() {
        let catalog = load_embedded_catalog();

        // Should have some modules
        assert!(!catalog.modules.is_empty());

        // Should have protobuf
        assert!(catalog.modules.contains_key("protobuf"));

        // Should have cmake mappings
        assert!(!catalog.cmake_mappings.is_empty());
    }

    #[test]
    fn test_catalog_has_expected_modules() {
        let catalog = load_embedded_catalog();

        // Check for key modules from the plan
        let expected_modules = [
            "protobuf",
            "fmt",
            "flatbuffers",
            "onetbb",
            "boringssl",
            "curl",
            "zlib",
            "libssh2",
        ];

        for module in expected_modules {
            assert!(
                catalog.modules.contains_key(module),
                "Missing expected module: {module}"
            );
        }
    }

    #[test]
    fn test_catalog_boost_components() {
        let catalog = load_embedded_catalog();

        // Boost should have component mappings
        let Some(mapping) = catalog.cmake_mappings.get("Boost") else {
            panic!("Boost mapping missing from embedded catalog");
        };
        assert!(mapping.components.contains_key("filesystem"));
        assert!(mapping.components.contains_key("system"));
        assert!(mapping.components.contains_key("asio"));
    }

    #[test]
    fn embedded_catalog_parses_without_the_empty_fallback() {
        let strict = strict_catalog();
        let loaded = load_embedded_catalog();

        assert!(!strict.modules.is_empty());
        assert!(!strict.cmake_mappings.is_empty());
        assert!(!strict.gradle_mappings.is_empty());
        assert_eq!(strict.modules.len(), loaded.modules.len());
        assert_eq!(strict.cmake_mappings.len(), loaded.cmake_mappings.len());
        assert_eq!(strict.gradle_mappings.len(), loaded.gradle_mappings.len());
    }

    #[test]
    fn embedded_modules_are_internally_consistent() {
        let catalog = strict_catalog();

        for (key, module) in &catalog.modules {
            assert_eq!(key, &module.name, "module key and name differ");
            assert!(!module.versions.is_empty(), "{key}: no versions listed");
            let unique: BTreeSet<&String> = module.versions.iter().collect();
            assert_eq!(
                unique.len(),
                module.versions.len(),
                "{key}: duplicate versions"
            );
            assert!(
                module.versions.contains(&module.latest_version),
                "{key}: latest_version {} is not among its versions",
                module.latest_version
            );
            for target in &module.targets {
                assert!(
                    label_belongs_to(&target.label, key),
                    "{key}: target label {} names another repository",
                    target.label
                );
            }
            for dependency in module.dependencies.keys() {
                assert!(
                    catalog.modules.contains_key(dependency),
                    "{key}: dependency {dependency} is not in the catalog"
                );
            }
        }
    }

    #[test]
    fn embedded_mappings_reference_catalog_modules_and_targets() {
        let catalog = strict_catalog();
        let exported: BTreeSet<&str> = catalog
            .modules
            .values()
            .flat_map(|module| module.targets.iter().map(|target| target.label.as_str()))
            .collect();
        let mappings = catalog
            .cmake_mappings
            .iter()
            .map(|(name, mapping)| ("cmake", name, mapping))
            .chain(
                catalog
                    .gradle_mappings
                    .iter()
                    .map(|(name, mapping)| ("gradle", name, mapping)),
            );

        for (kind, name, mapping) in mappings {
            // Umbrella packages such as Boost resolve through their components.
            if mapping.components.is_empty() {
                assert!(
                    catalog.modules.contains_key(&mapping.module),
                    "{kind} mapping {name}: module {} is not in the catalog",
                    mapping.module
                );
            }
            if let Some(target) = &mapping.target {
                assert!(
                    label_belongs_to(target, &mapping.module),
                    "{kind} mapping {name}: target {target} is outside module {}",
                    mapping.module
                );
                assert!(
                    exported.contains(target.as_str()),
                    "{kind} mapping {name}: target {target} is not exported by the catalog"
                );
            }
            for (component, label) in &mapping.components {
                assert!(
                    exported.contains(label.as_str()),
                    "{kind} mapping {name}: component {component} maps to {label}, which is not exported by the catalog"
                );
            }
        }
    }
}
