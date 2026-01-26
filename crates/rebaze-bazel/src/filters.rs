//! Filtering and sanitization for CMake -> Bazel migration.
//!
//! This module provides comprehensive filtering of platform-specific,
//! compiler-specific, and otherwise non-portable CMake artifacts.

use std::collections::HashSet;

/// Filter compile definitions (defines) for Bazel compatibility.
///
/// Removes:
/// - Windows-specific defines (_WIN32, WIN32, _MSC_VER, etc.)
/// - macOS-specific defines (__APPLE__, __MACH__, etc.)
/// - Linux-specific defines (__linux__, __unix__, etc.)
/// - MSVC runtime defines (_HAS_*, _CRT_*, _ITERATOR_DEBUG_LEVEL, etc.)
/// - Feature detection results that may not be portable (HAVE_*, HAS_*)
/// - Architecture-specific defines (_M_X64, __LP64__, etc.)
/// - Debug/Release configuration defines (handled by Bazel)
pub fn filter_defines(defines: &[String]) -> Vec<String> {
    defines
        .iter()
        .filter(|def| !is_problematic_define(def))
        .cloned()
        .collect()
}

fn is_problematic_define(def: &str) -> bool {
    // Extract just the define name (handle -DFOO=bar and FOO=bar formats)
    let name = def
        .strip_prefix("-D")
        .unwrap_or(def)
        .split('=')
        .next()
        .unwrap_or(def);

    // Windows-specific
    if matches!(
        name,
        "_WIN32"
            | "WIN32"
            | "_WIN64"
            | "WIN64"
            | "_WINDOWS"
            | "WINDOWS"
            | "_WINDLL"
            | "_USRDLL"
            | "__MINGW32__"
            | "__MINGW64__"
            | "__CYGWIN__"
            | "_MSC_VER"
            | "_MSC_FULL_VER"
            | "_MSC_BUILD"
            | "WINVER"
            | "_WIN32_WINNT"
            | "NOMINMAX"
            | "WIN32_LEAN_AND_MEAN"
    ) {
        return true;
    }

    // macOS-specific
    if matches!(name, "__APPLE__" | "__MACH__" | "TARGET_OS_MAC" | "TARGET_OS_IPHONE") {
        return true;
    }

    // Linux/Unix-specific
    if matches!(
        name,
        "__linux__" | "__unix__" | "__unix" | "__GLIBC__" | "_GNU_SOURCE" | "_POSIX_SOURCE"
    ) {
        return true;
    }

    // Architecture-specific
    if matches!(
        name,
        "_M_X64"
            | "_M_AMD64"
            | "_M_IX86"
            | "_M_ARM"
            | "_M_ARM64"
            | "__LP64__"
            | "__ILP32__"
            | "__x86_64__"
            | "__i386__"
            | "__aarch64__"
            | "__arm__"
    ) {
        return true;
    }

    // MSVC runtime/iterator defines
    if name.starts_with("_HAS_")
        || name.starts_with("_CRT_")
        || name.starts_with("_SCL_")
        || name.starts_with("_ITERATOR_DEBUG_LEVEL")
        || name.starts_with("_SECURE_SCL")
    {
        return true;
    }

    // Feature detection (may not be portable across platforms)
    // Be conservative - only filter clearly platform-specific ones
    if name.ends_with("_UNLOCKED") // e.g., HAVE_FWRITE_UNLOCKED - Linux-specific
        || name.starts_with("HAVE_PTHREAD_")  // Platform-specific pthread features
    {
        return true;
    }

    // Debug/Release defines (Bazel handles these via -c dbg/-c opt)
    if matches!(name, "_DEBUG" | "DEBUG" | "NDEBUG" | "_NDEBUG") {
        return true;
    }

    // DLL export/import macros (not meaningful in Bazel)
    if name.ends_with("_EXPORTS")
        || name.ends_with("_EXPORT")
        || name.ends_with("_DLL")
        || name.ends_with("_SHARED")
        || name.contains("DLL_EXPORT")
        || name.contains("DLL_IMPORT")
        || name.contains("DECLSPEC")
    {
        return true;
    }

    // External library preference defines (prefer bundled versions)
    // These cause issues when the external dependency isn't configured
    if name.ends_with("_EXTERNAL") || name.ends_with("_USE_EXTERNAL") {
        return true;
    }

    false
}

/// Filter compile options (copts) for Bazel compatibility.
///
/// Removes:
/// - MSVC flags (start with /)
/// - Optimization flags (-O*, /O*) - handled by Bazel
/// - Standard flags (-std=*, /std:*) - handled in .bazelrc
/// - Debug info flags (-g*, /Z*) - handled by Bazel
/// - Linker flags passed to compiler (-Wl,*)
/// - Include paths (-I*, -isystem*) - should be in includes attribute
/// - Warning flags that are too strict or compiler-specific
pub fn filter_copts(copts: &[String]) -> Vec<String> {
    copts
        .iter()
        .filter(|opt| !is_problematic_copt(opt))
        .cloned()
        .collect()
}

fn is_problematic_copt(opt: &str) -> bool {
    // MSVC flags
    if opt.starts_with('/') {
        return true;
    }

    // Optimization flags (let Bazel handle these)
    if opt.starts_with("-O") || opt == "-Os" || opt == "-Oz" || opt == "-Ofast" {
        return true;
    }

    // Debug info flags (let Bazel handle these)
    if opt.starts_with("-g") && (opt == "-g" || opt.starts_with("-g1") || opt.starts_with("-g2") || opt.starts_with("-g3") || opt == "-ggdb") {
        return true;
    }

    // Standard flags (should be in .bazelrc)
    if opt.starts_with("-std=") || opt.starts_with("--std=") {
        return true;
    }

    // Linker flags passed to compiler
    if opt.starts_with("-Wl,") {
        return true;
    }

    // Include paths (should be in includes attribute)
    if opt.starts_with("-I") || opt.starts_with("-isystem") {
        return true;
    }

    // Define flags (should be in defines attribute)
    if opt.starts_with("-D") {
        return true;
    }

    // Architecture-specific flags that break portability
    if opt.starts_with("-march=") || opt.starts_with("-mtune=") || opt.starts_with("-mcpu=") {
        return true;
    }

    // Link-time flags that shouldn't be in copts
    if opt.starts_with("-l") || opt.starts_with("-L") {
        return true;
    }

    // Empty or whitespace-only
    if opt.trim().is_empty() {
        return true;
    }

    // Exception handling flags (can cause ABI issues)
    // Let the project's actual code decide exception handling policy
    if opt == "-fno-exceptions" || opt == "-fexceptions" {
        return true;
    }

    // Runtime type info flags (can cause ABI issues)
    if opt == "-fno-rtti" || opt == "-frtti" {
        return true;
    }

    false
}

/// Filter source files for Bazel compatibility.
///
/// Removes:
/// - Absolute paths (not portable)
/// - Windows resource files (.rc, .res, .ico, .manifest)
/// - macOS-specific files (.plist, .xib, .storyboard)
/// - CMake build directory references
/// - Object files and archives
/// - Non-source files that shouldn't be in srcs
pub fn filter_sources(sources: &[String]) -> Vec<String> {
    sources
        .iter()
        .filter(|src| !is_problematic_source(src))
        .cloned()
        .collect()
}

fn is_problematic_source(src: &str) -> bool {
    // Absolute paths
    if src.starts_with('/') {
        return true;
    }

    // CMake build directory references
    if src.contains("CMAKE_CURRENT_BINARY_DIR")
        || src.contains("CMAKE_BINARY_DIR")
        || src.starts_with("${")
    {
        return true;
    }

    // Check file extension using std::path for proper case-insensitive handling
    let path = std::path::Path::new(src);
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        let ext_lower = ext.to_lowercase();

        // Windows resource/manifest files
        if matches!(ext_lower.as_str(), "rc" | "res" | "ico" | "manifest" | "def") {
            return true;
        }

        // macOS-specific files
        if matches!(ext_lower.as_str(), "plist" | "xib" | "storyboard" | "xcassets") {
            return true;
        }

        // Object files and archives (prebuilt)
        if matches!(ext_lower.as_str(), "o" | "obj" | "a" | "lib" | "so" | "dylib" | "dll") {
            return true;
        }

        // CMake script files
        if ext_lower == "cmake" {
            return true;
        }
    }

    false
}

/// Filter include directories for Bazel compatibility.
///
/// Removes:
/// - Absolute system paths (/usr/include, /opt/*, etc.)
/// - CMake variable references that can't be resolved
/// - Empty or whitespace-only paths
pub fn filter_includes(includes: &[String]) -> Vec<String> {
    includes
        .iter()
        .filter_map(|inc| normalize_and_filter_include(inc))
        .collect::<HashSet<_>>() // Deduplicate
        .into_iter()
        .collect()
}

fn normalize_and_filter_include(inc: &str) -> Option<String> {
    // Skip empty
    if inc.trim().is_empty() {
        return None;
    }

    // Expand common CMake variables
    let normalized = if inc.starts_with("${") {
        if let Some(end) = inc.find('}') {
            let var = &inc[2..end];
            let rest = &inc[end + 1..];

            match var {
                // Source directory variables - expand to relative path
                "CMAKE_CURRENT_SOURCE_DIR"
                | "CMAKE_SOURCE_DIR"
                | "PROJECT_SOURCE_DIR"
                | "CMAKE_CURRENT_LIST_DIR" => rest.trim_start_matches('/').to_string(),
                // Skip binary dir references (generated files)
                "CMAKE_CURRENT_BINARY_DIR" | "CMAKE_BINARY_DIR" | "PROJECT_BINARY_DIR" => {
                    return None;
                }
                // Unknown variable - skip
                _ => return None,
            }
        } else {
            return None;
        }
    } else {
        inc.to_string()
    };

    // Skip absolute system paths
    if normalized.starts_with("/usr/")
        || normalized.starts_with("/opt/")
        || normalized.starts_with("/Library/")
        || normalized.starts_with("/System/")
        || normalized.starts_with("C:\\")
        || normalized.starts_with("c:\\")
    {
        return None;
    }

    // Normalize current directory references
    let result = normalized
        .trim_start_matches("./")
        .trim_end_matches('/')
        .to_string();

    // Skip if empty after normalization
    if result.is_empty() || result == "." {
        return Some(".".to_string());
    }

    Some(result)
}

/// Comprehensive CMake to Bazel dependency mapping.
///
/// Maps common CMake package names and imported targets to their
/// Bazel equivalents. Returns None for system libraries that don't
/// need explicit deps (pthread, m, dl, etc.)
pub fn map_dependency(cmake_name: &str) -> Option<String> {
    // Handle -l flags from pkg-config
    if let Some(lib) = cmake_name.strip_prefix("-l") {
        return map_system_library(lib);
    }

    // Handle CMake imported targets (Package::Component)
    if cmake_name.contains("::") {
        return map_imported_target(cmake_name);
    }

    // Handle plain library names
    map_plain_library(cmake_name)
}

fn map_system_library(lib: &str) -> Option<String> {
    match lib {
        // Standard C/POSIX libraries (usually don't need explicit deps)
        "pthread" | "c" | "m" | "dl" | "rt" | "util" => None,

        // Compression
        "z" | "zlib" => Some("@zlib".to_string()),
        "bz2" | "bzip2" => Some("@bzip2".to_string()),
        "lzma" | "lz4" | "zstd" => Some(format!("@{lib}")),

        // Crypto/SSL
        "ssl" | "crypto" => Some("@openssl".to_string()),

        // XML/JSON
        "expat" => Some("@expat".to_string()),
        "xml2" => Some("@libxml2".to_string()),

        // Database
        "sqlite3" => Some("@sqlite3".to_string()),

        // Misc
        "curl" => Some("@curl".to_string()),
        "pcre" | "pcre2-8" => Some("@pcre".to_string()),

        // Unknown - generate comment
        _ => Some(format!("# TODO: Map -l{lib} to Bazel dependency")),
    }
}

fn map_imported_target(cmake_name: &str) -> Option<String> {
    let parts: Vec<&str> = cmake_name.split("::").collect();
    if parts.len() != 2 {
        return Some(format!("# TODO: Map {cmake_name} to Bazel"));
    }

    let package = parts[0].to_lowercase();
    let component = parts[1].to_lowercase();

    match package.as_str() {
        // Libraries that are implicit or bundled - no external dep needed
        "threads" | "fmt" | "spdlog" => None,

        // Boost
        "boost" => Some(format!("@boost//:{component}")),

        // OpenSSL - all components map to @openssl//:component
        "openssl" => Some(format!("@openssl//:{component}")),

        // ZLIB
        "zlib" => Some("@zlib".to_string()),

        // Protobuf
        "protobuf" => match component.as_str() {
            "protobuf" | "libprotobuf" => Some("@com_google_protobuf//:protobuf".to_string()),
            "protobuf-lite" | "libprotobuf-lite" => {
                Some("@com_google_protobuf//:protobuf_lite".to_string())
            }
            "protoc" => Some("@com_google_protobuf//:protoc".to_string()),
            _ => Some(format!("@com_google_protobuf//:{component}")),
        },

        // gRPC
        "grpc" => match component.as_str() {
            "grpc" => Some("@com_github_grpc_grpc//:grpc".to_string()),
            "grpc++" => Some("@com_github_grpc_grpc//:grpc++".to_string()),
            _ => Some(format!("@com_github_grpc_grpc//:{component}")),
        },

        // Abseil
        "absl" => Some(format!("@com_google_absl//absl/{component}")),

        // Google Test - all components map to @com_google_googletest//:component
        "gtest" | "googletest" => Some(format!("@com_google_googletest//:{component}")),

        // Google Benchmark - requires adding bazel_dep to MODULE.bazel
        "benchmark" => Some("# TODO: Add @com_google_benchmark (bazel_dep in MODULE.bazel)".to_string()),

        // nlohmann_json
        "nlohmann_json" => Some("@nlohmann_json//:json".to_string()),

        // CURL
        "curl" => Some("@curl".to_string()),

        // SQLite
        "sqlite" | "sqlite3" => Some("@sqlite3".to_string()),

        // PkgConfig (usually from pkg_check_modules)
        "pkgconfig" => Some(format!("//third_party:{component}")),

        // Unknown package
        _ => Some(format!("# TODO: Map {cmake_name} to Bazel")),
    }
}

fn map_plain_library(lib: &str) -> Option<String> {
    match lib.to_lowercase().as_str() {
        // Standard libraries (no explicit dep needed) + commonly bundled libs
        "pthread" | "threads::threads" | "m" | "dl" | "rt" | "c" | "fmt" | "spdlog" => None,

        // Common libraries with known Bazel targets
        "zlib" | "z" => Some("@zlib".to_string()),
        "openssl" | "ssl" | "crypto" => Some("@openssl".to_string()),
        "boost" => Some("@boost".to_string()),
        "protobuf" => Some("@com_google_protobuf//:protobuf".to_string()),
        "grpc" | "grpc++" => Some("@com_github_grpc_grpc//:grpc++".to_string()),

        // Test/benchmark libraries - require adding bazel_dep to MODULE.bazel
        "gtest" | "gtest_main" | "gmock" | "gmock_main" | "benchmark" => {
            Some(format!("# TODO: Add external dep for '{lib}' (bazel_dep in MODULE.bazel)"))
        }

        // Unknown - generate TODO comment
        _ => Some(format!("# TODO: Map '{lib}' to Bazel dependency")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_filter_defines() {
        let defines = vec![
            "MY_DEFINE".to_string(),
            "_WIN32".to_string(),
            "FEATURE_ENABLED=1".to_string(),
            "_HAS_EXCEPTIONS=0".to_string(),
            "NDEBUG".to_string(),
            "__linux__".to_string(),
            "MYLIB_EXPORTS".to_string(),
        ];

        let filtered = filter_defines(&defines);
        assert!(filtered.contains(&"MY_DEFINE".to_string()));
        assert!(filtered.contains(&"FEATURE_ENABLED=1".to_string()));
        assert!(!filtered.contains(&"_WIN32".to_string()));
        assert!(!filtered.contains(&"_HAS_EXCEPTIONS=0".to_string()));
        assert!(!filtered.contains(&"NDEBUG".to_string()));
        assert!(!filtered.contains(&"__linux__".to_string()));
        assert!(!filtered.contains(&"MYLIB_EXPORTS".to_string()));
    }

    #[test]
    fn test_filter_copts() {
        let copts = vec![
            "-Wall".to_string(),
            "/W4".to_string(),
            "-O2".to_string(),
            "-std=c++17".to_string(),
            "-fPIC".to_string(),
            "-I/usr/include".to_string(),
            "-DFOO".to_string(),
        ];

        let filtered = filter_copts(&copts);
        assert!(filtered.contains(&"-Wall".to_string()));
        assert!(filtered.contains(&"-fPIC".to_string()));
        assert!(!filtered.contains(&"/W4".to_string()));
        assert!(!filtered.contains(&"-O2".to_string()));
        assert!(!filtered.contains(&"-std=c++17".to_string()));
        assert!(!filtered.contains(&"-I/usr/include".to_string()));
        assert!(!filtered.contains(&"-DFOO".to_string()));
    }

    #[test]
    fn test_filter_sources() {
        let sources = vec![
            "src/main.cpp".to_string(),
            "/absolute/path.cpp".to_string(),
            "resource.rc".to_string(),
            "Info.plist".to_string(),
            "prebuilt.o".to_string(),
        ];

        let filtered = filter_sources(&sources);
        assert!(filtered.contains(&"src/main.cpp".to_string()));
        assert!(!filtered.contains(&"/absolute/path.cpp".to_string()));
        assert!(!filtered.contains(&"resource.rc".to_string()));
        assert!(!filtered.contains(&"Info.plist".to_string()));
        assert!(!filtered.contains(&"prebuilt.o".to_string()));
    }

    #[test]
    fn test_map_dependency() {
        assert_eq!(map_dependency("pthread"), None);
        assert_eq!(map_dependency("m"), None);
        assert_eq!(map_dependency("-lz"), Some("@zlib".to_string()));
        assert_eq!(
            map_dependency("Boost::system"),
            Some("@boost//:system".to_string())
        );
        assert_eq!(
            map_dependency("OpenSSL::SSL"),
            Some("@openssl//:ssl".to_string())
        );
        assert_eq!(map_dependency("Threads::Threads"), None);
        assert_eq!(
            map_dependency("GTest::gtest"),
            Some("@com_google_googletest//:gtest".to_string())
        );
    }
}
