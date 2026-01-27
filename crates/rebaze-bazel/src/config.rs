//! Configuration system for rebaze migrations.
//!
//! This module provides a flexible, extensible configuration system that allows
//! users to customize dependency mappings, Bazel versions, filter rules, and more.
//!
//! Configuration can be loaded from:
//! - Default embedded configuration
//! - Project-specific `rebaze.toml` file
//! - CLI overrides

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

/// Main configuration for CMake to Bazel migration.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct MigrationConfig {
    /// Bazel dependency versions
    pub versions: VersionConfig,

    /// Dependency mapping configuration
    pub mappings: MappingConfig,

    /// Filter configuration for sources, includes, defines, copts
    pub filters: FilterConfig,

    /// Build file generation options
    pub build: BuildConfig,

    /// Strategy configuration for third-party dependencies
    pub strategy: StrategyConfig,
}

#[allow(clippy::literal_string_with_formatting_args)]
const COMPONENT_PLACEHOLDER: &str = "{component}";

impl MigrationConfig {
    /// Load configuration from a TOML file, merging with defaults.
    pub fn load_from_file(path: &Path) -> anyhow::Result<Self> {
        let content = std::fs::read_to_string(path)?;
        let config: Self = toml::from_str(&content)?;
        Ok(config)
    }

    /// Try to load configuration from the project directory.
    /// Looks for `rebaze.toml` in the given directory.
    pub fn load_from_project(project_dir: &Path) -> Self {
        let config_path = project_dir.join("rebaze.toml");
        if config_path.exists() {
            match Self::load_from_file(&config_path) {
                Ok(config) => {
                    tracing::info!("Loaded configuration from {}", config_path.display());
                    return config;
                }
                Err(e) => {
                    tracing::warn!(
                        "Failed to load {}: {}. Using defaults.",
                        config_path.display(),
                        e
                    );
                }
            }
        }
        Self::default()
    }

    /// Get the Bazel target for a system library.
    pub fn map_system_library(&self, lib: &str) -> Option<String> {
        // Check explicit mappings first
        if let Some(target) = self.mappings.system_libraries.get(lib) {
            if target.is_empty() {
                return None; // Empty string means "skip this library"
            }
            return Some(target.clone());
        }

        // Check if it's an implicit system library (pthread, m, c, etc.)
        if self
            .mappings
            .implicit_system_libs
            .contains(&lib.to_string())
        {
            return None;
        }

        // No mapping found
        None
    }

    /// Get the Bazel target for a CMake imported target (e.g., Boost::filesystem).
    pub fn map_imported_target(&self, package: &str, component: &str) -> Option<String> {
        // Check package-specific mappings
        if let Some(pkg_config) = self.mappings.packages.get(package) {
            // Check component-specific mapping
            if let Some(target) = pkg_config.components.get(component) {
                return Some(target.clone());
            }
            // Use default target pattern with component substitution
            return Some(
                pkg_config
                    .target_pattern
                    .replace(COMPONENT_PLACEHOLDER, component),
            );
        }
        None
    }

    /// Unified dependency mapping that handles all CMake dependency formats.
    ///
    /// Handles:
    /// - `-l` flags from pkg-config (e.g., "-lz")
    /// - Imported targets (e.g., "Package::Component")
    /// - Plain library names (e.g., "gtest")
    ///
    /// Returns `None` for ignored libraries or implicit system libs.
    /// Returns `Some(target)` for mapped dependencies.
    /// Returns `Some("# TODO: ...")` for unknown dependencies.
    pub fn map_dependency(&self, cmake_name: &str) -> Option<String> {
        // 1. Check if this library should be ignored
        if self.is_ignored_library(cmake_name) {
            return None;
        }

        // 2. Handle -l flags from pkg-config
        if let Some(lib) = cmake_name.strip_prefix("-l") {
            return self.map_system_library(lib);
        }

        // 3. Handle CMake imported targets (Package::Component)
        if cmake_name.contains("::") {
            return self.map_imported_target_full(cmake_name);
        }

        // 4. Handle plain library names
        self.map_plain_library(cmake_name)
    }

    /// Check if a library should be ignored (bundled or implicit).
    pub fn is_ignored_library(&self, name: &str) -> bool {
        // Check exact match
        if self.mappings.ignored_libraries.contains(&name.to_string()) {
            return true;
        }

        // Check Package::Component format
        if name.contains("::") {
            let parts: Vec<&str> = name.split("::").collect();
            if parts.len() == 2 {
                let package = parts[0].to_lowercase();
                // Check if package itself is ignored (e.g., "fmt", "spdlog")
                if self.mappings.ignored_libraries.contains(&package) {
                    return true;
                }
            }
        }

        false
    }

    /// Map an imported target in full "Package::Component" format.
    pub fn map_imported_target_full(&self, cmake_name: &str) -> Option<String> {
        let parts: Vec<&str> = cmake_name.split("::").collect();
        if parts.len() != 2 {
            return Some(format!("# TODO: Map {cmake_name} to Bazel"));
        }

        let package = parts[0];
        let component = parts[1];

        // Try exact package match first, then lowercase
        if let Some(target) = self.map_imported_target(package, component) {
            return Some(target);
        }

        // Try case-insensitive matching for common packages
        let package_lower = package.to_lowercase();
        let component_lower = component.to_lowercase();

        // Check if this is a commonly ignored package
        if matches!(package_lower.as_str(), "threads" | "fmt" | "spdlog") {
            return None;
        }

        // Try to find a matching package (case-insensitive)
        for (pkg_name, pkg_config) in &self.mappings.packages {
            if pkg_name.to_lowercase() == package_lower {
                // Check component-specific mapping
                for (comp_name, target) in &pkg_config.components {
                    if comp_name.to_lowercase() == component_lower {
                        return Some(target.clone());
                    }
                }
                // Use default pattern
                return Some(
                    pkg_config
                        .target_pattern
                        .replace(COMPONENT_PLACEHOLDER, &component_lower),
                );
            }
        }

        // No mapping found
        Some(format!("# TODO: Map {cmake_name} to Bazel"))
    }

    /// Map a plain library name to a Bazel target.
    pub fn map_plain_library(&self, lib: &str) -> Option<String> {
        // Check exact match in plain_libraries
        if let Some(target) = self.mappings.plain_libraries.get(lib) {
            return Some(target.clone());
        }

        // Check case-insensitive match
        let lib_lower = lib.to_lowercase();
        for (name, target) in &self.mappings.plain_libraries {
            if name.to_lowercase() == lib_lower {
                return Some(target.clone());
            }
        }

        // Check if it's an implicit system library
        if self
            .mappings
            .implicit_system_libs
            .contains(&lib.to_string())
        {
            return None;
        }

        // Check system libraries as fallback
        if let Some(target) = self.map_system_library(lib) {
            return Some(target);
        }

        // Unknown library - generate TODO comment
        Some(format!("# TODO: Map '{lib}' to Bazel dependency"))
    }

    /// Get transitive dependencies for a package.
    pub fn get_transitive_deps(&self, pkg_name: &str) -> Vec<String> {
        self.mappings
            .transitive_deps
            .get(pkg_name)
            .cloned()
            .unwrap_or_default()
    }

    /// Get system linkopts for packages (fallback when pkg-config unavailable).
    pub fn get_system_linkopts(&self, packages: &[String]) -> Vec<String> {
        let mut linkopts = Vec::new();
        for pkg in packages {
            // Strip version constraints (e.g., "glib-2.0>=2.44.0" -> "glib-2.0")
            let base = pkg.split(">=").next().unwrap_or(pkg).trim();

            if let Some(opts) = self.mappings.system_linkopts.get(base) {
                linkopts.extend(opts.clone());
            } else {
                // Fallback: derive linkopt from package name
                let lib_name = base
                    .strip_prefix("lib")
                    .unwrap_or(base)
                    .split('-')
                    .next()
                    .unwrap_or(base);
                linkopts.push(format!("-l{lib_name}"));
            }
        }
        linkopts
    }

    /// Get known package info for building from source.
    pub fn get_known_package(&self, name: &str) -> Option<&KnownPackageInfo> {
        // Try exact match first
        if let Some(info) = self.mappings.known_packages.get(name) {
            return Some(info);
        }

        // Try normalized name (e.g., "glib-2.0" -> "glib")
        let normalized = name
            .strip_suffix("-2.0")
            .or_else(|| name.strip_suffix("-1.0"))
            .unwrap_or(name);
        self.mappings.known_packages.get(normalized)
    }
}

/// Bazel dependency versions.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct VersionConfig {
    pub rules_cc: String,
    pub platforms: String,
    pub rules_foreign_cc: String,
    pub googletest: String,
    pub google_benchmark: String,
}

impl Default for VersionConfig {
    fn default() -> Self {
        Self {
            rules_cc: "0.1.1".to_string(),
            platforms: "0.0.11".to_string(),
            rules_foreign_cc: "0.14.0".to_string(),
            googletest: "1.15.2".to_string(),
            google_benchmark: "1.9.1".to_string(),
        }
    }
}

/// Dependency mapping configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct MappingConfig {
    /// System library to Bazel target mappings.
    /// Empty string means "skip this library" (implicit system lib).
    pub system_libraries: BTreeMap<String, String>,

    /// Libraries that are implicitly provided by the system and don't need mapping.
    pub implicit_system_libs: Vec<String>,

    /// CMake package configurations.
    pub packages: BTreeMap<String, PackageMapping>,

    /// Plain library name mappings (e.g., "gtest" -> "@googletest//:gtest").
    /// Used when libraries are referenced by name without -l prefix or :: separator.
    #[serde(default)]
    pub plain_libraries: BTreeMap<String, String>,

    /// Libraries to ignore completely (bundled or implicit).
    /// These won't generate any dependency entry.
    #[serde(default)]
    pub ignored_libraries: Vec<String>,

    /// Transitive dependencies for packages.
    /// E.g., `"glib" -> ["pcre2", "libffi", "zlib"]`
    #[serde(default)]
    pub transitive_deps: BTreeMap<String, Vec<String>>,

    /// System linkopts for packages (fallback when pkg-config unavailable).
    /// E.g., `"glib-2.0" -> ["-lglib-2.0"]`
    #[serde(default)]
    pub system_linkopts: BTreeMap<String, Vec<String>>,

    /// Known package metadata for building from source.
    #[serde(default)]
    pub known_packages: BTreeMap<String, KnownPackageInfo>,
}

impl Default for MappingConfig {
    fn default() -> Self {
        Self {
            system_libraries: Self::default_system_libraries(),
            implicit_system_libs: Self::default_implicit_system_libs(),
            packages: Self::default_packages(),
            plain_libraries: Self::default_plain_libraries(),
            ignored_libraries: Self::default_ignored_libraries(),
            transitive_deps: Self::default_transitive_deps(),
            system_linkopts: Self::default_system_linkopts(),
            known_packages: Self::default_known_packages(),
        }
    }
}

impl MappingConfig {
    fn default_system_libraries() -> BTreeMap<String, String> {
        [
            // Compression libraries
            ("z", "@zlib"),
            ("zlib", "@zlib"),
            ("bz2", "@bzip2"),
            ("bzip2", "@bzip2"),
            ("lzma", "@lzma"),
            ("lz4", "@lz4"),
            ("zstd", "@zstd"),
            // Crypto/SSL
            ("ssl", "@openssl"),
            ("crypto", "@openssl"),
            ("openssl", "@openssl"),
            // XML/JSON
            ("expat", "@expat"),
            ("xml2", "@libxml2"),
            // Database
            ("sqlite3", "@sqlite3"),
            // Network
            ("curl", "@curl"),
            // Regex
            ("pcre", "@pcre"),
            ("pcre2-8", "@pcre"),
            // Testing
            ("gtest", "@googletest//:gtest"),
            ("gtest_main", "@googletest//:gtest_main"),
            ("gmock", "@googletest//:gmock"),
            ("gmock_main", "@googletest//:gmock_main"),
            ("benchmark", "@google_benchmark//:benchmark"),
            ("benchmark_main", "@google_benchmark//:benchmark"),
            // Protobuf/gRPC
            ("protobuf", "@com_google_protobuf//:protobuf"),
            ("grpc", "@com_github_grpc_grpc//:grpc"),
            ("grpc++", "@com_github_grpc_grpc//:grpc++"),
        ]
        .into_iter()
        .map(|(name, target)| (name.to_string(), target.to_string()))
        .collect()
    }

    fn default_implicit_system_libs() -> Vec<String> {
        [
            "pthread", "c", "m", "dl", "rt", "util", "resolv", "nsl", "socket",
        ]
        .into_iter()
        .map(str::to_string)
        .collect()
    }

    fn default_packages() -> BTreeMap<String, PackageMapping> {
        let mut packages = BTreeMap::new();

        packages.insert(
            "Boost".to_string(),
            package_mapping("@boost//:{component}", &[]),
        );

        packages.insert(
            "OpenSSL".to_string(),
            package_mapping(
                "@openssl//:{component}",
                &[("SSL", "@openssl//:ssl"), ("Crypto", "@openssl//:crypto")],
            ),
        );

        packages.insert("ZLIB".to_string(), package_mapping("@zlib", &[]));

        packages.insert(
            "Protobuf".to_string(),
            package_mapping(
                "@com_google_protobuf//:{component}",
                &[
                    ("protobuf", "@com_google_protobuf//:protobuf"),
                    ("protobuf_lite", "@com_google_protobuf//:protobuf_lite"),
                    ("protoc", "@com_google_protobuf//:protoc"),
                ],
            ),
        );

        packages.insert(
            "gRPC".to_string(),
            package_mapping(
                "@com_github_grpc_grpc//:{component}",
                &[
                    ("grpc", "@com_github_grpc_grpc//:grpc"),
                    ("grpc++", "@com_github_grpc_grpc//:grpc++"),
                ],
            ),
        );

        packages.insert(
            "absl".to_string(),
            package_mapping("@com_google_absl//absl/{component}", &[]),
        );

        packages.insert(
            "GTest".to_string(),
            package_mapping(
                "@googletest//:{component}",
                &[
                    ("gtest", "@googletest//:gtest"),
                    ("gtest_main", "@googletest//:gtest_main"),
                    ("gmock", "@googletest//:gmock"),
                    ("gmock_main", "@googletest//:gmock_main"),
                ],
            ),
        );

        packages.insert(
            "googletest".to_string(),
            package_mapping("@googletest//:{component}", &[]),
        );

        packages.insert(
            "benchmark".to_string(),
            package_mapping("@google_benchmark//:{component}", &[]),
        );

        packages.insert(
            "nlohmann_json".to_string(),
            package_mapping("@nlohmann_json//:json", &[]),
        );

        packages.insert(
            "PkgConfig".to_string(),
            package_mapping("//third_party:{component}", &[]),
        );

        packages
    }

    fn default_plain_libraries() -> BTreeMap<String, String> {
        [
            // Testing frameworks
            ("gtest", "@googletest//:gtest"),
            ("gtest_main", "@googletest//:gtest_main"),
            ("gmock", "@googletest//:gmock"),
            ("gmock_main", "@googletest//:gmock_main"),
            ("benchmark", "@google_benchmark//:benchmark"),
            ("benchmark_main", "@google_benchmark//:benchmark"),
            // Common libraries
            ("zlib", "@zlib"),
            ("openssl", "@openssl"),
            ("boost", "@boost"),
            ("protobuf", "@com_google_protobuf//:protobuf"),
            ("grpc", "@com_github_grpc_grpc//:grpc++"),
            ("grpc++", "@com_github_grpc_grpc//:grpc++"),
        ]
        .into_iter()
        .map(|(name, target)| (name.to_string(), target.to_string()))
        .collect()
    }

    fn default_ignored_libraries() -> Vec<String> {
        [
            // Libraries that are commonly bundled or don't need explicit deps
            "fmt",
            "spdlog",
            // Thread library is handled by toolchain
            "Threads::Threads",
        ]
        .into_iter()
        .map(str::to_string)
        .collect()
    }

    fn default_transitive_deps() -> BTreeMap<String, Vec<String>> {
        [
            ("glib", vec!["pcre2", "libffi", "zlib"]),
            ("glib-2.0", vec!["pcre2", "libffi", "zlib"]),
            ("gobject", vec!["glib", "libffi", "pcre2", "zlib"]),
            ("gobject-2.0", vec!["glib", "libffi", "pcre2", "zlib"]),
        ]
        .into_iter()
        .map(|(name, deps)| {
            (
                name.to_string(),
                deps.into_iter().map(str::to_string).collect(),
            )
        })
        .collect()
    }

    fn default_system_linkopts() -> BTreeMap<String, Vec<String>> {
        [
            ("glib-2.0", vec!["-lglib-2.0"]),
            ("gobject-2.0", vec!["-lgobject-2.0"]),
            ("gio-2.0", vec!["-lgio-2.0"]),
            ("gthread-2.0", vec!["-lgthread-2.0"]),
            ("gmodule-2.0", vec!["-lgmodule-2.0"]),
            ("libpeas-1.0", vec!["-lpeas-1.0"]),
            ("libdnf", vec!["-ldnf"]),
            ("smartcols", vec!["-lsmartcols"]),
            ("libcurl", vec!["-lcurl"]),
            ("curl", vec!["-lcurl"]),
            ("openssl", vec!["-lssl", "-lcrypto"]),
            ("zlib", vec!["-lz"]),
            ("libxml-2.0", vec!["-lxml2"]),
        ]
        .into_iter()
        .map(|(name, opts)| {
            (
                name.to_string(),
                opts.into_iter().map(str::to_string).collect(),
            )
        })
        .collect()
    }

    fn default_known_packages() -> BTreeMap<String, KnownPackageInfo> {
        let mut packages = BTreeMap::new();

        packages.insert(
            "glib".to_string(),
            KnownPackageInfo {
                version: "2.82.4".to_string(),
                url: "https://download.gnome.org/sources/glib/2.82/glib-2.82.4.tar.xz".to_string(),
                sha256: "937f1312d7a883e6fdd0f75ed28de2f9e4fc2ee83fece0ab46a5153d5c34ce93"
                    .to_string(),
                strip_prefix: "glib-2.82.4".to_string(),
                build_system: "meson".to_string(),
                out_static_libs: vec!["libglib-2.0.a".to_string()],
                out_shared_libs: vec![
                    "libglib-2.0.so".to_string(),
                    "libglib-2.0.dylib".to_string(),
                ],
                deps: vec![
                    "pcre2".to_string(),
                    "libffi".to_string(),
                    "zlib".to_string(),
                ],
                build_options: vec![
                    "-Dtests=false".to_string(),
                    "-Dglib_debug=disabled".to_string(),
                    "-Dintrospection=disabled".to_string(),
                ],
            },
        );

        packages.insert(
            "gobject".to_string(),
            KnownPackageInfo {
                version: "2.82.4".to_string(),
                url: "https://download.gnome.org/sources/glib/2.82/glib-2.82.4.tar.xz".to_string(),
                sha256: "937f1312d7a883e6fdd0f75ed28de2f9e4fc2ee83fece0ab46a5153d5c34ce93"
                    .to_string(),
                strip_prefix: "glib-2.82.4".to_string(),
                build_system: "meson".to_string(),
                out_static_libs: vec!["libgobject-2.0.a".to_string()],
                out_shared_libs: vec![
                    "libgobject-2.0.so".to_string(),
                    "libgobject-2.0.dylib".to_string(),
                ],
                deps: vec!["glib".to_string(), "libffi".to_string()],
                build_options: vec!["-Dtests=false".to_string()],
            },
        );

        packages.insert(
            "pcre2".to_string(),
            KnownPackageInfo {
                version: "10.44".to_string(),
                url: "https://github.com/PCRE2Project/pcre2/releases/download/pcre2-10.44/pcre2-10.44.tar.gz".to_string(),
                sha256: "d34f02e113cf7193f1b06f1c26c1f8e7fa6e8a9fe1c3b1c29e579f5a6a6e8a19".to_string(),
                strip_prefix: "pcre2-10.44".to_string(),
                build_system: "cmake".to_string(),
                out_static_libs: vec!["libpcre2-8.a".to_string()],
                out_shared_libs: vec![],
                deps: vec![],
                build_options: vec![],
            },
        );

        packages.insert(
            "zlib".to_string(),
            KnownPackageInfo {
                version: "1.3.1".to_string(),
                url: "https://github.com/madler/zlib/releases/download/v1.3.1/zlib-1.3.1.tar.gz"
                    .to_string(),
                sha256: "9a93b2b7dfdac77ceba5a558a580e74667dd6fede4585b91eefb60f03b72df23"
                    .to_string(),
                strip_prefix: "zlib-1.3.1".to_string(),
                build_system: "cmake".to_string(),
                out_static_libs: vec!["libz.a".to_string()],
                out_shared_libs: vec![],
                deps: vec![],
                build_options: vec![],
            },
        );

        packages.insert(
            "libffi".to_string(),
            KnownPackageInfo {
                version: "3.4.6".to_string(),
                url:
                    "https://github.com/libffi/libffi/releases/download/v3.4.6/libffi-3.4.6.tar.gz"
                        .to_string(),
                sha256: "b0dea9df23c863a7a50e825440f3ebffabd65df1497108e5d437747843895a4e"
                    .to_string(),
                strip_prefix: "libffi-3.4.6".to_string(),
                build_system: "autotools".to_string(),
                out_static_libs: vec!["libffi.a".to_string()],
                out_shared_libs: vec![],
                deps: vec![],
                build_options: vec![],
            },
        );

        packages
    }
}

/// Mapping configuration for a CMake package.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackageMapping {
    /// Default target pattern. Use {component} as placeholder.
    pub target_pattern: String,

    /// Component-specific target overrides.
    #[serde(default)]
    pub components: BTreeMap<String, String>,
}

fn package_mapping(target_pattern: &str, components: &[(&str, &str)]) -> PackageMapping {
    let components = components
        .iter()
        .map(|(name, target)| (String::from(*name), String::from(*target)))
        .collect();

    PackageMapping {
        target_pattern: target_pattern.to_string(),
        components,
    }
}

/// Metadata for a known package that can be built from source.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnownPackageInfo {
    /// Package version.
    pub version: String,

    /// Source tarball URL.
    pub url: String,

    /// SHA256 checksum of the tarball.
    pub sha256: String,

    /// Directory prefix to strip from tarball.
    pub strip_prefix: String,

    /// Build system: "cmake", "meson", or "autotools".
    pub build_system: String,

    /// Static libraries produced by the build.
    #[serde(default)]
    pub out_static_libs: Vec<String>,

    /// Shared libraries produced by the build.
    #[serde(default)]
    pub out_shared_libs: Vec<String>,

    /// Dependencies required to build this package.
    #[serde(default)]
    pub deps: Vec<String>,

    /// Build-system-specific options (meson options, cmake args, etc.).
    #[serde(default)]
    pub build_options: Vec<String>,
}

/// Filter configuration for cleaning up CMake artifacts.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct FilterConfig {
    /// File extensions to exclude from sources.
    pub excluded_source_extensions: Vec<String>,

    /// Define patterns to filter out.
    pub excluded_define_patterns: Vec<String>,

    /// Compiler option patterns to filter out.
    pub excluded_copt_patterns: Vec<String>,

    /// Include path patterns to filter out.
    pub excluded_include_patterns: Vec<String>,
}

impl Default for FilterConfig {
    fn default() -> Self {
        Self {
            excluded_source_extensions: vec![
                // Windows
                "rc".to_string(),
                "res".to_string(),
                "ico".to_string(),
                "manifest".to_string(),
                "def".to_string(),
                // macOS
                "plist".to_string(),
                "xib".to_string(),
                "storyboard".to_string(),
                "xcassets".to_string(),
                // Prebuilt
                "o".to_string(),
                "obj".to_string(),
                "a".to_string(),
                "lib".to_string(),
                "so".to_string(),
                "dylib".to_string(),
                "dll".to_string(),
                // Build system
                "cmake".to_string(),
            ],
            excluded_define_patterns: vec![
                // Windows
                "_WIN32".to_string(),
                "WIN32".to_string(),
                "_WIN64".to_string(),
                "_WINDOWS".to_string(),
                "_MSC_VER".to_string(),
                "NOMINMAX".to_string(),
                // macOS
                "__APPLE__".to_string(),
                "__MACH__".to_string(),
                // Linux
                "__linux__".to_string(),
                "_GNU_SOURCE".to_string(),
                // Debug
                "_DEBUG".to_string(),
                "NDEBUG".to_string(),
            ],
            excluded_copt_patterns: vec![
                // MSVC
                "/".to_string(),
                // Optimization
                "-O".to_string(),
                // Debug
                "-g".to_string(),
                // Standard (handled by toolchain)
                "-std=".to_string(),
                // Architecture
                "-march=".to_string(),
                "-mtune=".to_string(),
            ],
            excluded_include_patterns: vec![
                "/usr/".to_string(),
                "/opt/".to_string(),
                "/Library/".to_string(),
                "/System/".to_string(),
            ],
        }
    }
}

/// Build file generation options.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct BuildConfig {
    /// Default visibility for generated targets.
    pub default_visibility: String,

    /// Header file glob patterns.
    pub header_patterns: Vec<String>,

    /// Default C++ standard.
    pub default_cxx_standard: String,

    /// Default C standard.
    pub default_c_standard: String,

    /// Module version for generated MODULE.bazel.
    pub module_version: String,

    /// Custom rule configuration (like Spotify's spotify_library instead of cc_library).
    #[serde(default)]
    pub rules: RuleConfig,
}

impl Default for BuildConfig {
    fn default() -> Self {
        Self {
            default_visibility: "//visibility:public".to_string(),
            header_patterns: vec!["**/*.h".to_string(), "**/*.hpp".to_string()],
            default_cxx_standard: "17".to_string(),
            default_c_standard: "11".to_string(),
            module_version: "0.1.0".to_string(),
            rules: RuleConfig::default(),
        }
    }
}

/// Custom rule configuration for generated BUILD files.
///
/// Allows organizations to use their own rule wrappers (e.g., spotify_library)
/// instead of standard Bazel rules (cc_library).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct RuleConfig {
    /// Rule name for libraries (default: "cc_library").
    pub library_rule: String,

    /// Rule name for binaries (default: "cc_binary").
    pub binary_rule: String,

    /// Rule name for tests (default: "cc_test").
    pub test_rule: String,

    /// Extra load statements needed for custom rules.
    #[serde(default)]
    pub extra_loads: Vec<LoadStatement>,
}

impl Default for RuleConfig {
    fn default() -> Self {
        Self {
            library_rule: "cc_library".to_string(),
            binary_rule: "cc_binary".to_string(),
            test_rule: "cc_test".to_string(),
            extra_loads: Vec::new(),
        }
    }
}

/// A Starlark load statement.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoadStatement {
    /// The .bzl file to load from (e.g., `@spotify//build:rules.bzl`).
    pub bzl: String,

    /// The items to import (e.g., `["spotify_library", "spotify_binary"]`).
    pub items: Vec<String>,
}

/// Third-party dependency strategy configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct StrategyConfig {
    /// Default strategy for resolving dependencies.
    /// Options: "bcr", "system", "source", "conan", "vcpkg", "prebuilt", "custom"
    pub default: String,

    /// Fallback strategy when BCR resolution fails.
    /// Only used when default is "bcr".
    #[serde(default)]
    pub bcr_fallback: Option<String>,

    /// Per-package strategy overrides.
    #[serde(default)]
    pub overrides: BTreeMap<String, String>,

    /// BCR-specific configuration.
    #[serde(default)]
    pub bcr: BcrConfig,
}

impl Default for StrategyConfig {
    fn default() -> Self {
        Self {
            default: "system".to_string(),
            bcr_fallback: None,
            overrides: BTreeMap::new(),
            bcr: BcrConfig::default(),
        }
    }
}

/// Configuration specific to BCR (Bazel Central Registry) strategy.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct BcrConfig {
    /// Override BCR module names for specific packages.
    /// E.g., `{"openssl": "boringssl"}` to use boringssl instead of openssl.
    #[serde(default)]
    pub module_overrides: BTreeMap<String, String>,

    /// Pin specific BCR module versions.
    /// E.g., `{"protobuf": "27.5"}` to use a specific version.
    #[serde(default)]
    pub version_pins: BTreeMap<String, String>,

    /// Packages to skip BCR resolution for (force fallback).
    #[serde(default)]
    pub skip_packages: Vec<String>,
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = MigrationConfig::default();
        assert_eq!(config.versions.rules_cc, "0.1.1");
        assert_eq!(config.strategy.default, "system");
    }

    #[test]
    fn test_system_library_mapping() {
        let config = MigrationConfig::default();

        // Known library
        assert_eq!(config.map_system_library("zlib"), Some("@zlib".to_string()));

        // Implicit system library
        assert_eq!(config.map_system_library("pthread"), None);

        // Unknown library
        assert_eq!(config.map_system_library("unknown_lib"), None);
    }

    #[test]
    fn test_imported_target_mapping() {
        let config = MigrationConfig::default();

        // OpenSSL with component
        assert_eq!(
            config.map_imported_target("OpenSSL", "SSL"),
            Some("@openssl//:ssl".to_string())
        );

        // GTest
        assert_eq!(
            config.map_imported_target("GTest", "gtest"),
            Some("@googletest//:gtest".to_string())
        );

        // Boost with dynamic component
        assert_eq!(
            config.map_imported_target("Boost", "filesystem"),
            Some("@boost//:filesystem".to_string())
        );
    }

    #[test]
    fn test_toml_parsing() {
        let toml_content = r#"
[versions]
rules_cc = "0.3.0"
googletest = "1.16.0"

[mappings.system_libraries]
mylib = "@my_repo//:mylib"

[strategy]
default = "source"
"#;
        let config: MigrationConfig = toml::from_str(toml_content).unwrap();
        assert_eq!(config.versions.rules_cc, "0.3.0");
        assert_eq!(config.versions.googletest, "1.16.0");
        assert_eq!(config.strategy.default, "source");
        assert_eq!(
            config.mappings.system_libraries.get("mylib"),
            Some(&"@my_repo//:mylib".to_string())
        );
    }

    #[test]
    fn test_map_dependency_unified() {
        let config = MigrationConfig::default();

        // -l flags
        assert_eq!(config.map_dependency("-lz"), Some("@zlib".to_string()));
        assert_eq!(config.map_dependency("-lpthread"), None);

        // Imported targets (Package::Component)
        assert_eq!(
            config.map_dependency("OpenSSL::SSL"),
            Some("@openssl//:ssl".to_string())
        );
        assert_eq!(
            config.map_dependency("Boost::filesystem"),
            Some("@boost//:filesystem".to_string())
        );

        // Ignored libraries
        assert_eq!(config.map_dependency("Threads::Threads"), None);
        assert_eq!(config.map_dependency("fmt"), None);

        // Plain library names
        assert_eq!(
            config.map_dependency("gtest"),
            Some("@googletest//:gtest".to_string())
        );
    }

    #[test]
    fn test_is_ignored_library() {
        let config = MigrationConfig::default();

        // Exact match
        assert!(config.is_ignored_library("fmt"));
        assert!(config.is_ignored_library("spdlog"));

        // Package::Component with ignored package
        assert!(config.is_ignored_library("fmt::fmt"));

        // Not ignored
        assert!(!config.is_ignored_library("gtest"));
        assert!(!config.is_ignored_library("Boost::filesystem"));
    }

    #[test]
    fn test_map_plain_library() {
        let config = MigrationConfig::default();

        // Known libraries
        assert_eq!(
            config.map_plain_library("gtest"),
            Some("@googletest//:gtest".to_string())
        );
        assert_eq!(
            config.map_plain_library("benchmark"),
            Some("@google_benchmark//:benchmark".to_string())
        );

        // System library fallback
        assert_eq!(config.map_plain_library("pthread"), None);

        // Unknown generates TODO
        let result = config.map_plain_library("unknown_lib");
        assert!(result.is_some());
        assert!(result.unwrap().contains("TODO"));
    }

    #[test]
    fn test_get_transitive_deps() {
        let config = MigrationConfig::default();

        let deps = config.get_transitive_deps("glib");
        assert!(deps.contains(&"pcre2".to_string()));
        assert!(deps.contains(&"libffi".to_string()));
        assert!(deps.contains(&"zlib".to_string()));

        // Unknown package returns empty
        assert!(config.get_transitive_deps("unknown").is_empty());
    }

    #[test]
    fn test_get_system_linkopts() {
        let config = MigrationConfig::default();

        // Known package
        let linkopts = config.get_system_linkopts(&["glib-2.0".to_string()]);
        assert!(linkopts.contains(&"-lglib-2.0".to_string()));

        // Package with version constraint
        let linkopts = config.get_system_linkopts(&["glib-2.0>=2.44.0".to_string()]);
        assert!(linkopts.contains(&"-lglib-2.0".to_string()));

        // OpenSSL (multiple linkopts)
        let linkopts = config.get_system_linkopts(&["openssl".to_string()]);
        assert!(linkopts.contains(&"-lssl".to_string()));
        assert!(linkopts.contains(&"-lcrypto".to_string()));
    }

    #[test]
    fn test_get_known_package() {
        let config = MigrationConfig::default();

        // Exact match
        let info = config.get_known_package("zlib");
        assert!(info.is_some());
        assert_eq!(info.unwrap().build_system, "cmake");

        // Normalized match (strip -2.0 suffix)
        let info = config.get_known_package("glib-2.0");
        assert!(info.is_some());
        assert_eq!(info.unwrap().build_system, "meson");

        // Unknown
        assert!(config.get_known_package("unknown").is_none());
    }

    #[test]
    fn test_rule_config_defaults() {
        let config = MigrationConfig::default();

        assert_eq!(config.build.rules.library_rule, "cc_library");
        assert_eq!(config.build.rules.binary_rule, "cc_binary");
        assert_eq!(config.build.rules.test_rule, "cc_test");
        assert!(config.build.rules.extra_loads.is_empty());
    }

    #[test]
    fn test_custom_rules_toml_parsing() {
        let toml_content = r#"
[build.rules]
library_rule = "spotify_library"
binary_rule = "spotify_binary"

[[build.rules.extra_loads]]
bzl = "@spotify//build:rules.bzl"
items = ["spotify_library", "spotify_binary"]
"#;
        let config: MigrationConfig = toml::from_str(toml_content).unwrap();
        assert_eq!(config.build.rules.library_rule, "spotify_library");
        assert_eq!(config.build.rules.binary_rule, "spotify_binary");
        assert_eq!(config.build.rules.extra_loads.len(), 1);
        assert_eq!(
            config.build.rules.extra_loads[0].bzl,
            "@spotify//build:rules.bzl"
        );
    }
}
