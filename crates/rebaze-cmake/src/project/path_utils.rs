//! Path manipulation utilities for CMake to Bazel migration.
//!
//! Handles normalization of CMake paths and variable substitutions
//! to produce Bazel-compatible relative paths.

/// Prefix a path with a subdirectory, handling "." specially.
pub fn prefix_path(subdir: &str, path: &str) -> String {
    if path == "." {
        subdir.to_string()
    } else if let Some(stripped) = path.strip_prefix("./") {
        format!("{subdir}/{stripped}")
    } else {
        format!("{subdir}/{path}")
    }
}

/// Normalize include path for Bazel compatibility.
///
/// Handles common CMake variables and path patterns:
/// - `${CMAKE_CURRENT_SOURCE_DIR}` -> "."
/// - `${CMAKE_SOURCE_DIR}` -> "."
/// - `${PROJECT_SOURCE_DIR}` -> "."
/// - Leading `/` (non-system paths) -> stripped
pub fn normalize_include_path(path: &str) -> String {
    // Handle ${CMAKE_CURRENT_SOURCE_DIR} - replace with "."
    if path == "${CMAKE_CURRENT_SOURCE_DIR}" {
        return ".".to_string();
    }
    // Handle ${CMAKE_CURRENT_SOURCE_DIR}/subpath
    if let Some(rest) = path.strip_prefix("${CMAKE_CURRENT_SOURCE_DIR}/") {
        return rest.to_string();
    }
    // Handle ${CMAKE_SOURCE_DIR} similarly
    if path == "${CMAKE_SOURCE_DIR}" {
        return ".".to_string();
    }
    if let Some(rest) = path.strip_prefix("${CMAKE_SOURCE_DIR}/") {
        return rest.to_string();
    }
    // Handle ${PROJECT_SOURCE_DIR} - same as CMAKE_SOURCE_DIR for most projects
    if path == "${PROJECT_SOURCE_DIR}" {
        return ".".to_string();
    }
    if let Some(rest) = path.strip_prefix("${PROJECT_SOURCE_DIR}/") {
        return rest.to_string();
    }
    // Handle paths that start with / but aren't absolute system paths
    // This can happen when a CMake variable expands to empty, leaving "/subdir"
    if let Some(rest) = path.strip_prefix('/') {
        // Only strip if it's not a system path
        if !is_system_path(path) {
            return rest.to_string();
        }
    }
    path.to_string()
}

/// Check if a path is a system include path that shouldn't be normalized.
fn is_system_path(path: &str) -> bool {
    path.starts_with("/usr/")
        || path.starts_with("/opt/")
        || path.starts_with("/lib")
        || path.starts_with("/System/")
        || path.starts_with("/Library/")
}

/// Strip -D or /D prefix from a define string.
pub fn strip_define_prefix(value: &str) -> String {
    if let Some(stripped) = value.strip_prefix("-D") {
        return stripped.to_string();
    }
    if let Some(stripped) = value.strip_prefix("/D") {
        return stripped.to_string();
    }
    value.to_string()
}

/// Check if a value is an include flag that should be filtered.
pub fn is_include_flag(value: &str) -> bool {
    matches!(value, "-I" | "-isystem" | "/I")
        || value.starts_with("-I")
        || value.starts_with("-isystem")
        || value.starts_with("/I")
}

/// Extend a vector with unique items only.
pub fn extend_unique(target: &mut Vec<String>, items: Vec<String>) {
    for item in items {
        if !target.contains(&item) {
            target.push(item);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_prefix_path_dot() {
        assert_eq!(prefix_path("subdir", "."), "subdir");
    }

    #[test]
    fn test_prefix_path_relative() {
        assert_eq!(prefix_path("subdir", "./foo"), "subdir/foo");
        assert_eq!(prefix_path("subdir", "foo"), "subdir/foo");
    }

    #[test]
    fn test_normalize_cmake_current_source_dir() {
        assert_eq!(normalize_include_path("${CMAKE_CURRENT_SOURCE_DIR}"), ".");
        assert_eq!(
            normalize_include_path("${CMAKE_CURRENT_SOURCE_DIR}/include"),
            "include"
        );
    }

    #[test]
    fn test_normalize_project_source_dir() {
        assert_eq!(normalize_include_path("${PROJECT_SOURCE_DIR}"), ".");
        assert_eq!(normalize_include_path("${PROJECT_SOURCE_DIR}/src"), "src");
    }

    #[test]
    fn test_normalize_keeps_system_paths() {
        assert_eq!(normalize_include_path("/usr/include"), "/usr/include");
        assert_eq!(
            normalize_include_path("/opt/local/include"),
            "/opt/local/include"
        );
    }

    #[test]
    fn test_normalize_strips_leading_slash_non_system() {
        assert_eq!(normalize_include_path("/include"), "include");
        assert_eq!(normalize_include_path("/src/foo"), "src/foo");
    }

    #[test]
    fn test_strip_define_prefix() {
        assert_eq!(strip_define_prefix("-DFOO"), "FOO");
        assert_eq!(strip_define_prefix("/DBAR"), "BAR");
        assert_eq!(strip_define_prefix("BAZ"), "BAZ");
    }

    #[test]
    fn test_is_include_flag() {
        assert!(is_include_flag("-I"));
        assert!(is_include_flag("-isystem"));
        assert!(is_include_flag("/I"));
        assert!(is_include_flag("-I/usr/include"));
        assert!(!is_include_flag("-Wall"));
    }

    #[test]
    fn test_extend_unique() {
        let mut vec = vec!["a".to_string(), "b".to_string()];
        extend_unique(&mut vec, vec!["b".to_string(), "c".to_string()]);
        assert_eq!(vec, vec!["a", "b", "c"]);
    }
}
