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
        let boost_mapping = catalog.cmake_mappings.get("Boost");
        assert!(boost_mapping.is_some());

        let mapping = boost_mapping.expect("Boost mapping exists");
        assert!(mapping.components.contains_key("filesystem"));
        assert!(mapping.components.contains_key("system"));
        assert!(mapping.components.contains_key("asio"));
    }
}
