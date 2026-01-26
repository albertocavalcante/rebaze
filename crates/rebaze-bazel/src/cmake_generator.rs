//! Bazel file generation from CMake projects.

use std::collections::BTreeSet;

use rebaze_cmake::{CMakeProject, Executable, Library, LibraryKind};

use crate::filters::{filter_copts, filter_defines, filter_includes, filter_sources, map_dependency};
use crate::starlark::{BazelDep, CcBinary, CcLibrary, Glob, Load, Module, Package, SrcsWithHdrs};

/// Get known transitive dependencies for a package.
fn get_known_transitive_deps(pkg_name: &str) -> Vec<&'static str> {
    match pkg_name {
        "glib" | "glib-2.0" => vec!["pcre2", "libffi", "zlib"],
        "gobject" | "gobject-2.0" => vec!["glib", "libffi", "pcre2", "zlib"],
        _ => vec![],
    }
}

/// Detected external dependencies that need bazel_dep entries.
#[derive(Debug, Default)]
struct DetectedDeps {
    googletest: bool,
    benchmark: bool,
}

/// Detect external test/benchmark frameworks from link libraries.
fn detect_external_deps(project: &CMakeProject) -> DetectedDeps {
    let mut deps = DetectedDeps::default();

    // Check all executables and libraries for test framework usage
    let all_link_libs = project
        .executables
        .iter()
        .flat_map(|e| e.link_libraries.iter())
        .chain(project.libraries.iter().flat_map(|l| l.link_libraries.iter()));

    for lib in all_link_libs {
        let lib_lower = lib.to_lowercase();
        if lib_lower.contains("gtest") || lib_lower.contains("gmock") || lib_lower.contains("googletest") {
            deps.googletest = true;
        }
        if lib_lower.contains("benchmark") && !lib_lower.contains("google") {
            deps.benchmark = true;
        }
    }

    // Also check CMake packages
    for pkg in &project.packages {
        let pkg_lower = pkg.name.to_lowercase();
        if pkg_lower.contains("gtest") || pkg_lower.contains("googletest") {
            deps.googletest = true;
        }
        if pkg_lower == "benchmark" {
            deps.benchmark = true;
        }
    }

    deps
}

use crate::config::MigrationConfig;

/// Generate a MODULE.bazel file for a CMake project.
#[must_use]
pub fn generate_module_bazel(project: &CMakeProject, config: &MigrationConfig) -> String {
    let mut parts = Vec::new();

    // Add docstring
    parts.push(format!(
        "\"\"\"Bazel module for {} - migrated from CMake by rebaze.\"\"\"",
        project.name
    ));

    // Module declaration
    let module = Module {
        name: project.name.to_lowercase().replace('-', "_"),
        version: config.build.module_version.clone(),
    };
    parts.push(
        crate::starlark::serde_starlark::to_string(&module).unwrap_or_else(|e| format!("# Error: {e}")),
    );

    // C/C++ toolchain dependency
    parts.push("# C/C++ toolchain".to_string());
    let rules_cc = BazelDep {
        name: "rules_cc".to_string(),
        version: config.versions.rules_cc.clone(),
    };
    parts.push(
        crate::starlark::serde_starlark::to_string(&rules_cc).unwrap_or_else(|e| format!("# Error: {e}")),
    );

    // Detect and add external test/benchmark dependencies
    let detected = detect_external_deps(project);
    if detected.googletest {
        parts.push("# Testing framework (auto-detected from CMake)".to_string());
        let googletest = BazelDep {
            name: "googletest".to_string(),
            version: config.versions.googletest.clone(),
        };
        parts.push(
            crate::starlark::serde_starlark::to_string(&googletest).unwrap_or_else(|e| format!("# Error: {e}")),
        );
    }
    if detected.benchmark {
        parts.push("# Benchmarking framework (auto-detected from CMake)".to_string());
        let benchmark = BazelDep {
            name: "google_benchmark".to_string(),
            version: config.versions.google_benchmark.clone(),
        };
        parts.push(
            crate::starlark::serde_starlark::to_string(&benchmark).unwrap_or_else(|e| format!("# Error: {e}")),
        );
    }

    // Add platform support if needed
    if !project.packages.is_empty() {
        parts.push("# Platform and dependency management".to_string());
        let platforms = BazelDep {
            name: "platforms".to_string(),
            version: config.versions.platforms.clone(),
        };
        parts.push(
            crate::starlark::serde_starlark::to_string(&platforms).unwrap_or_else(|e| format!("# Error: {e}")),
        );
    }

    // Add rules_foreign_cc for pkg-config dependencies
    if !project.pkg_config_modules.is_empty() {
        parts.push("# Foreign build system support (for building deps from source)".to_string());
        let rules_foreign_cc = BazelDep {
            name: "rules_foreign_cc".to_string(),
            version: config.versions.rules_foreign_cc.clone(),
        };
        parts.push(
            crate::starlark::serde_starlark::to_string(&rules_foreign_cc).unwrap_or_else(|e| format!("# Error: {e}")),
        );

        // Collect all package names including transitive deps for source builds
        let mut all_source_packages = std::collections::BTreeSet::new();
        for pkg in &project.pkg_config_modules {
            let name = pkg.prefix.to_lowercase().replace('-', "_");
            all_source_packages.insert(name.clone());
            // Add known transitive deps
            for dep in get_known_transitive_deps(&name) {
                all_source_packages.insert(dep.to_string());
            }
        }

        // Add system_deps module extension for system library wrappers
        // Use BTreeSet to deduplicate and maintain consistent ordering
        let system_dep_names: Vec<String> = project
            .pkg_config_modules
            .iter()
            .map(|pkg| format!("system_{}", pkg.prefix.to_lowercase().replace('-', "_")))
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();

        parts.push("# System library wrappers (strategy = \"system\", the default)".to_string());
        parts.push("# NOTE: These wrap system-installed libraries - adjust paths in third_party/system_deps.bzl".to_string());
        parts.push(format!(
            r#"system_deps = use_extension("//third_party:system_deps.bzl", "system_deps")
use_repo(system_deps, {})"#,
            system_dep_names
                .iter()
                .map(|n| format!("\"{n}\""))
                .collect::<Vec<_>>()
                .join(", ")
        ));

        // Add source_deps module extension for building from source
        let source_dep_names: Vec<String> = all_source_packages
            .iter()
            .map(|name| format!("{name}_src"))
            .collect();

        parts.push("# Source downloads (strategy = \"source\" - hermetic builds)".to_string());
        parts.push("# NOTE: Uncomment to enable building dependencies from source".to_string());
        parts.push(format!(
            r#"# source_deps = use_extension("//third_party:source.bzl", "source_deps")
# use_repo(source_deps, {})"#,
            source_dep_names
                .iter()
                .map(|n| format!("\"{n}\""))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }

    parts.join("\n\n") + "\n"
}

/// Generate the root BUILD.bazel file.
#[must_use]
pub fn generate_root_build(project: &CMakeProject) -> String {
    let mut parts = Vec::new();

    // Add header comment with review notes
    let pkg_note = if project.pkg_config_modules.is_empty() {
        ""
    } else {
        "\n#   - pkg-config deps in //third_party/ use system libs (not hermetic)"
    };
    parts.push(format!(
        r"# BUILD file for {} - migrated from CMake by rebaze
#
# NOTE: This file was auto-generated and should be reviewed:
#   - glob patterns use allow_empty=True (some patterns may match nothing)
#   - include paths are inferred from source file locations{}",
        project.name, pkg_note
    ));

    // Load statement
    let mut items = BTreeSet::new();
    items.insert("cc_binary".to_string());
    items.insert("cc_library".to_string());
    let load = Load {
        bzl: "@rules_cc//cc:defs.bzl".to_string(),
        items,
    };
    parts.push(crate::starlark::serde_starlark::to_string(&load).unwrap_or_else(|e| format!("# Error: {e}")));

    // Add pkg-config dependency note
    if !project.pkg_config_modules.is_empty() {
        let mut pkg_comments = vec![
            "# pkg-config dependencies (wrappers in //third_party):".to_string(),
        ];
        for pkg in &project.pkg_config_modules {
            pkg_comments.push(format!("#   //third_party:{} -> {}",
                pkg.prefix.to_lowercase().replace('-', "_"),
                pkg.packages.join(", ")));
        }
        parts.push(pkg_comments.join("\n"));
    }

    // Package visibility
    let package = Package {
        default_visibility: vec!["//visibility:public".to_string()],
    };
    parts.push(
        crate::starlark::serde_starlark::to_string(&package).unwrap_or_else(|e| format!("# Error: {e}")),
    );

    // Generate libraries first (they may be dependencies of executables)
    // Deduplicate by name - CMake may report both shared and static variants
    let mut seen_libs = std::collections::BTreeSet::new();
    for lib in &project.libraries {
        let target_name = lib.name.replace('-', "_");
        if seen_libs.insert(target_name) {
            let cc_lib = build_cc_library(lib);
            parts.push(
                crate::starlark::serde_starlark::to_string(&cc_lib).unwrap_or_else(|e| format!("# Error: {e}")),
            );
        }
    }

    // Generate executables
    for exe in &project.executables {
        let cc_bin = build_cc_binary(exe, &project.libraries, &project.pkg_config_modules);
        parts.push(
            crate::starlark::serde_starlark::to_string(&cc_bin).unwrap_or_else(|e| format!("# Error: {e}")),
        );
    }

    parts.join("\n\n") + "\n"
}

/// Build a CcLibrary struct from a CMake Library.
fn build_cc_library(lib: &Library) -> CcLibrary {
    let target_name = lib.name.replace('-', "_");

    // Determine if this is header-only
    let is_header_only = lib.kind == LibraryKind::Interface || lib.sources.is_empty();

    let (srcs, hdrs, includes) = if is_header_only {
        // Header-only library
        (
            Vec::new(),
            Some(Glob {
                include: vec!["include/**/*.h".to_string(), "include/**/*.hpp".to_string()],
                exclude: Vec::new(),
                allow_empty: true, // Not all projects have .hpp files
            }),
            vec!["include".to_string()],
        )
    } else {
        // Regular library with sources - apply comprehensive filtering
        let includes = if lib.include_directories.is_empty() {
            vec!["include".to_string()]
        } else {
            filter_includes(&lib.include_directories)
        };

        (
            filter_sources(&lib.sources),
            Some(Glob {
                include: vec!["include/**/*.h".to_string(), "include/**/*.hpp".to_string()],
                exclude: Vec::new(),
                allow_empty: true, // Not all projects have .hpp files
            }),
            includes,
        )
    };

    // Map link dependencies using comprehensive mapper (filters out # TODO comments)
    // Deduplicate deps - CMake may report the same dependency multiple ways (e.g., fmt and fmt::fmt)
    let deps: Vec<String> = {
        let mut seen = std::collections::BTreeSet::new();
        lib.link_libraries
            .iter()
            .filter_map(|dep| map_dependency(dep))
            .filter(|dep| !dep.starts_with('#'))
            .filter(|dep| seen.insert(dep.clone()))
            .collect()
    };

    CcLibrary {
        name: target_name,
        srcs,
        hdrs,
        includes,
        deps,
        defines: filter_defines(&lib.compile_definitions),
        copts: filter_copts(&lib.compile_options),
        linkopts: Vec::new(),
        // For shared libraries, set linkstatic = False (default is True)
        linkstatic: if lib.kind == LibraryKind::Shared {
            Some(false)
        } else {
            None
        },
    }
}

/// Build a CcBinary struct from a CMake Executable.
fn build_cc_binary(
    exe: &Executable,
    libs: &[Library],
    pkg_config_modules: &[rebaze_cmake::PkgConfigModule],
) -> CcBinary {
    let target_name = exe.name.replace('-', "_");

    // Filter sources first
    let filtered_sources = filter_sources(&exe.sources);

    // Build a set of internal library names for O(1) lookups (avoid O(n²))
    let internal_lib_names: std::collections::BTreeSet<&str> =
        libs.iter().map(|l| l.name.as_str()).collect();

    // Start with explicit include directories (filtered)
    let mut includes_set: std::collections::BTreeSet<String> =
        filter_includes(&exe.include_directories).into_iter().collect();

    // Add unique directories containing source files as include paths
    // This mimics CMake's behavior where files can include headers from their own directory
    // Using BTreeSet for O(log n) insert instead of O(n) contains check
    for src in &filtered_sources {
        if let Some(dir) = std::path::Path::new(src).parent() {
            let dir_str = dir.to_string_lossy();
            if !dir_str.is_empty() {
                includes_set.insert(dir_str.into_owned());
            }
        }
    }

    // Convert to sorted Vec (BTreeSet already sorted)
    let includes: Vec<String> = includes_set.into_iter().collect();

    // Find minimal set of root directories for header globs
    // This avoids redundant patterns like dnf/**/*.h AND dnf/plugins/foo/**/*.h
    let glob_roots = find_minimal_glob_roots(&includes);

    // Generate header glob patterns - uses allow_empty since not all projects have .hpp files
    let hdrs_glob: Vec<String> = glob_roots
        .iter()
        .flat_map(|dir| vec![format!("{dir}/**/*.h"), format!("{dir}/**/*.hpp")])
        .collect();

    // Collect dependencies from link_libraries using comprehensive mapper
    // Deduplicate deps deterministically with BTreeSet
    let mut seen_deps = std::collections::BTreeSet::new();
    let mut deps: Vec<String> = exe
        .link_libraries
        .iter()
        .filter_map(|link_lib| {
            // Check if it's an internal library (handle both "spdlog" and "spdlog::spdlog" formats)
            let lib_name = if link_lib.contains("::") {
                // Extract the component name (e.g., "spdlog::spdlog_header_only" -> "spdlog_header_only")
                link_lib.split("::").last().unwrap_or(link_lib)
            } else {
                link_lib.as_str()
            };

            // O(log n) lookup instead of O(n)
            if internal_lib_names.contains(lib_name) {
                Some(format!(":{}", lib_name.replace('-', "_")))
            } else {
                map_dependency(link_lib)
            }
        })
        .filter(|dep| !dep.starts_with('#'))
        .filter(|dep| seen_deps.insert(dep.clone()))
        .collect();

    // Add pkg-config dependencies from third_party/
    for pkg in pkg_config_modules {
        let dep_name = pkg.prefix.to_lowercase().replace('-', "_");
        let dep = format!("//third_party:{dep_name}");
        if seen_deps.insert(dep.clone()) {
            deps.push(dep);
        }
    }

    CcBinary {
        name: target_name,
        srcs: SrcsWithHdrs {
            files: filtered_sources,
            hdrs_glob: if hdrs_glob.is_empty() { None } else { Some(hdrs_glob) },
        },
        includes,
        deps,
        defines: filter_defines(&exe.compile_definitions),
        copts: filter_copts(&exe.compile_options),
    }
}

/// Find the minimal set of root directories that cover all paths.
/// For example, given `["dnf", "dnf/plugins/foo", "dnf/plugins/bar"]`,
/// returns just `["dnf"]` since `dnf/**/*` covers all subdirectories.
fn find_minimal_glob_roots(dirs: &[String]) -> Vec<String> {
    if dirs.is_empty() {
        return Vec::new();
    }

    let mut roots: Vec<String> = Vec::new();

    for dir in dirs {
        // Check if this directory is already covered by an existing root
        let is_covered = roots.iter().any(|root| {
            dir.starts_with(root) && (dir.len() == root.len() || dir[root.len()..].starts_with('/'))
        });

        if !is_covered {
            // Remove any existing roots that this directory would cover
            roots.retain(|root| {
                !(root.starts_with(dir)
                    && (root.len() == dir.len() || root[dir.len()..].starts_with('/')))
            });
            roots.push(dir.clone());
        }
    }

    roots.sort();
    roots
}

#[cfg(test)]
mod tests {
    use super::*;
    use rebaze_cmake::Package;
    use std::path::PathBuf;

    fn test_project() -> CMakeProject {
        CMakeProject {
            name: "test-project".to_string(),
            version: Some("1.0.0".to_string()),
            languages: vec!["CXX".to_string()],
            cmake_minimum_version: Some("3.20".to_string()),
            path: PathBuf::from("."),
            executables: vec![Executable {
                name: "myapp".to_string(),
                sources: vec!["src/main.cpp".to_string()],
                link_libraries: vec!["mylib".to_string()],
                include_directories: vec![],
                compile_definitions: vec!["APP_DEF".to_string()],
                compile_options: vec!["-g".to_string()],
            }],
            libraries: vec![Library {
                name: "mylib".to_string(),
                kind: LibraryKind::Static,
                sources: vec!["src/lib.cpp".to_string()],
                link_libraries: vec![],
                include_directories: vec!["include".to_string()],
                compile_definitions: vec!["LIB_DEF".to_string()],
                compile_options: vec!["-O2".to_string()],
            }],
            packages: vec![Package {
                name: "Boost".to_string(),
                version: Some("1.70".to_string()),
                required: true,
                components: vec!["system".to_string()],
            }],
            pkg_config_modules: vec![],
            subdirectories: vec![],
            global_include_directories: vec![],
            cxx_standard: Some("17".to_string()),
            c_standard: None,
        }
    }

    #[test]
    fn test_generate_module_bazel() {
        let project = test_project();
        let config = crate::config::MigrationConfig::default();
        let content = generate_module_bazel(&project, &config);

        assert!(content.contains("module("));
        assert!(content.contains("test_project"));
        assert!(content.contains("rules_cc"));
        assert!(content.contains("platforms"));
    }

    #[test]
    fn test_generate_root_build() {
        let project = test_project();
        let content = generate_root_build(&project);

        assert!(content.contains("cc_library("));
        assert!(content.contains("cc_binary("));
        assert!(content.contains("mylib"));
        assert!(content.contains("myapp"));
        assert!(content.contains("LIB_DEF"));
        assert!(content.contains("APP_DEF"));
        // Optimization and debug flags are filtered out (Bazel handles these)
        assert!(!content.contains("-O2"), "optimization flags should be filtered");
        assert!(!content.contains("\"-g\""), "debug info flags should be filtered");
    }

    #[test]
    fn test_generate_module_bazel_format() {
        let project = test_project();
        let config = crate::config::MigrationConfig::default();
        let content = generate_module_bazel(&project, &config);

        // Check docstring is present
        assert!(content.contains("\"\"\"Bazel module for"));

        // Check module format
        assert!(content.contains("name = \"test_project\""));
        assert!(content.contains("version = \"0.1.0\""));

        // Check bazel_dep format
        assert!(content.contains("bazel_dep("));
        assert!(content.contains("name = \"rules_cc\""));
    }

    #[test]
    fn test_generate_root_build_format() {
        let project = test_project();
        let content = generate_root_build(&project);

        // Check load statement
        assert!(content.contains("load("));
        assert!(content.contains("@rules_cc//cc:defs.bzl"));

        // Check package visibility
        assert!(content.contains("package("));
        assert!(content.contains("default_visibility"));
        assert!(content.contains("//visibility:public"));

        // Check glob pattern for headers
        assert!(content.contains("glob("));
        assert!(content.contains("include/**/*.h"));
    }

    #[test]
    fn test_output_format_visual() {
        let project = test_project();
        let config = crate::config::MigrationConfig::default();

        let module = generate_module_bazel(&project, &config);
        let build = generate_root_build(&project);

        // This test verifies that the output is properly formatted Starlark
        // by checking the overall structure
        eprintln!("=== MODULE.bazel ===\n{module}");
        eprintln!("=== BUILD.bazel ===\n{build}");

        // Verify proper function call formatting
        assert!(
            module.contains("module(\n") || module.contains("module(name"),
            "module should have proper function call format"
        );
        assert!(
            build.contains("cc_library(\n") || build.contains("cc_library(name"),
            "cc_library should have proper function call format"
        );
        assert!(
            build.contains("cc_binary(\n") || build.contains("cc_binary(name"),
            "cc_binary should have proper function call format"
        );
    }
}
