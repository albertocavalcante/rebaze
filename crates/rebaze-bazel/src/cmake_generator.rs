//! Bazel file generation from CMake projects.

use std::collections::BTreeSet;

use rebaze_cmake::{CMakeProject, Executable, Library, LibraryKind};

use crate::starlark::{BazelDep, CcBinary, CcLibrary, Glob, Load, Module, Package, SrcsWithHdrs};

/// Get known transitive dependencies for a package.
fn get_known_transitive_deps(pkg_name: &str) -> Vec<&'static str> {
    match pkg_name {
        "glib" | "glib-2.0" => vec!["pcre2", "libffi", "zlib"],
        "gobject" | "gobject-2.0" => vec!["glib", "libffi", "pcre2", "zlib"],
        _ => vec![],
    }
}

/// Generate a MODULE.bazel file for a CMake project.
#[must_use]
pub fn generate_module_bazel(project: &CMakeProject) -> String {
    let mut parts = Vec::new();

    // Add docstring
    parts.push(format!(
        "\"\"\"Bazel module for {} - migrated from CMake by rebaze.\"\"\"",
        project.name
    ));

    // Module declaration
    let module = Module {
        name: project.name.replace('-', "_"),
        version: "0.1.0".to_string(),
    };
    parts.push(
        serde_starlark::to_string(&module).unwrap_or_else(|e| format!("# Error: {e}")),
    );

    // C/C++ toolchain dependency
    parts.push("# C/C++ toolchain".to_string());
    let rules_cc = BazelDep {
        name: "rules_cc".to_string(),
        version: "0.2.14".to_string(),
    };
    parts.push(
        serde_starlark::to_string(&rules_cc).unwrap_or_else(|e| format!("# Error: {e}")),
    );

    // Add platform support if needed
    if !project.packages.is_empty() {
        parts.push("# Platform and dependency management".to_string());
        let platforms = BazelDep {
            name: "platforms".to_string(),
            version: "1.0.0".to_string(),
        };
        parts.push(
            serde_starlark::to_string(&platforms).unwrap_or_else(|e| format!("# Error: {e}")),
        );
    }

    // Add rules_foreign_cc for pkg-config dependencies
    if !project.pkg_config_modules.is_empty() {
        parts.push("# Foreign build system support (for building deps from source)".to_string());
        let rules_foreign_cc = BazelDep {
            name: "rules_foreign_cc".to_string(),
            version: "0.15.1".to_string(),
        };
        parts.push(
            serde_starlark::to_string(&rules_foreign_cc).unwrap_or_else(|e| format!("# Error: {e}")),
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
        let system_dep_names: Vec<String> = project
            .pkg_config_modules
            .iter()
            .map(|pkg| format!("system_{}", pkg.prefix.to_lowercase().replace('-', "_")))
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
    parts.push(serde_starlark::to_string(&load).unwrap_or_else(|e| format!("# Error: {e}")));

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
        serde_starlark::to_string(&package).unwrap_or_else(|e| format!("# Error: {e}")),
    );

    // Generate libraries first (they may be dependencies of executables)
    // Deduplicate by name - CMake may report both shared and static variants
    let mut seen_libs = std::collections::HashSet::new();
    for lib in &project.libraries {
        let target_name = lib.name.replace('-', "_");
        if seen_libs.insert(target_name) {
            let cc_lib = build_cc_library(lib);
            parts.push(
                serde_starlark::to_string(&cc_lib).unwrap_or_else(|e| format!("# Error: {e}")),
            );
        }
    }

    // Generate executables
    for exe in &project.executables {
        let cc_bin = build_cc_binary(exe, &project.libraries, &project.pkg_config_modules);
        parts.push(
            serde_starlark::to_string(&cc_bin).unwrap_or_else(|e| format!("# Error: {e}")),
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
        // Regular library with sources
        let includes = if lib.include_directories.is_empty() {
            vec!["include".to_string()]
        } else {
            lib.include_directories
                .iter()
                .map(|d| normalize_include_dir(d))
                .collect()
        };

        (
            lib.sources.clone(),
            Some(Glob {
                include: vec!["include/**/*.h".to_string(), "include/**/*.hpp".to_string()],
                exclude: Vec::new(),
                allow_empty: true, // Not all projects have .hpp files
            }),
            includes,
        )
    };

    // Map link dependencies (filter out # TODO comments - those aren't valid labels)
    let deps: Vec<String> = lib
        .link_libraries
        .iter()
        .filter_map(|dep| map_cmake_dependency(dep))
        .filter(|dep| !dep.starts_with('#'))
        .collect();

    // Filter out invalid sources (absolute paths, Windows resource files)
    let filtered_srcs: Vec<String> = srcs
        .into_iter()
        .filter(|s| !s.starts_with('/') && !s.ends_with(".rc"))
        .collect();

    // Filter out MSVC-specific flags (start with /) - not portable to Unix
    let filtered_copts: Vec<String> = lib
        .compile_options
        .iter()
        .filter(|opt| !opt.starts_with('/'))
        .cloned()
        .collect();

    // Filter out Windows-specific defines
    let filtered_defines: Vec<String> = lib
        .compile_definitions
        .iter()
        .filter(|def| !def.starts_with("_HAS_") && !def.starts_with("_CRT_"))
        .cloned()
        .collect();

    CcLibrary {
        name: target_name,
        srcs: filtered_srcs,
        hdrs,
        includes,
        deps,
        defines: filtered_defines,
        copts: filtered_copts,
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

    // Start with explicit include directories
    let mut includes: Vec<String> = exe
        .include_directories
        .iter()
        .map(|d| normalize_include_dir(d))
        .collect();

    // Add unique directories containing source files as include paths
    // This mimics CMake's behavior where files can include headers from their own directory
    for src in &exe.sources {
        if let Some(dir) = std::path::Path::new(src).parent() {
            let dir_str = dir.to_string_lossy().to_string();
            if !dir_str.is_empty() && !includes.contains(&dir_str) {
                includes.push(dir_str);
            }
        }
    }

    // Sort for deterministic output
    includes.sort();
    includes.dedup();

    // Find minimal set of root directories for header globs
    // This avoids redundant patterns like dnf/**/*.h AND dnf/plugins/foo/**/*.h
    let glob_roots = find_minimal_glob_roots(&includes);

    // Generate header glob patterns - uses allow_empty since not all projects have .hpp files
    let hdrs_glob: Vec<String> = glob_roots
        .iter()
        .flat_map(|dir| vec![format!("{dir}/**/*.h"), format!("{dir}/**/*.hpp")])
        .collect();

    // Collect dependencies from link_libraries (filter out # TODO comments)
    let mut deps: Vec<String> = exe
        .link_libraries
        .iter()
        .filter_map(|link_lib| {
            // Check if it's an internal library
            if libs.iter().any(|l| l.name == *link_lib) {
                Some(format!(":{}", link_lib.replace('-', "_")))
            } else {
                map_cmake_dependency(link_lib)
            }
        })
        .filter(|dep| !dep.starts_with('#'))
        .collect();

    // Add pkg-config dependencies from third_party/
    for pkg in pkg_config_modules {
        let dep_name = pkg.prefix.to_lowercase().replace('-', "_");
        deps.push(format!("//third_party:{dep_name}"));
    }

    // Filter out MSVC-specific flags (start with /) - not portable to Unix
    let filtered_copts: Vec<String> = exe
        .compile_options
        .iter()
        .filter(|opt| !opt.starts_with('/'))
        .cloned()
        .collect();

    // Filter out Windows-specific defines
    let filtered_defines: Vec<String> = exe
        .compile_definitions
        .iter()
        .filter(|def| !def.starts_with("_HAS_") && !def.starts_with("_CRT_"))
        .cloned()
        .collect();

    CcBinary {
        name: target_name,
        srcs: SrcsWithHdrs {
            files: exe.sources.clone(),
            hdrs_glob: if hdrs_glob.is_empty() { None } else { Some(hdrs_glob) },
        },
        includes,
        deps,
        defines: filtered_defines,
        copts: filtered_copts,
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

/// Map CMake package/library names to Bazel dependencies.
fn map_cmake_dependency(cmake_name: &str) -> Option<String> {
    // Handle -l flags from pkg-config
    if let Some(lib) = cmake_name.strip_prefix("-l") {
        return Some(format!("# TODO: Map system library '{lib}'"));
    }

    // Handle CMake imported targets like Boost::system, OpenSSL::SSL
    if cmake_name.contains("::") {
        let parts: Vec<&str> = cmake_name.split("::").collect();
        if parts.len() == 2 {
            let package = parts[0].to_lowercase();
            let component = parts[1].to_lowercase();

            return match package.as_str() {
                "boost" => Some(format!("@boost//:{component}")),
                "openssl" => Some(format!("@openssl//:{component}")),
                "threads" => Some("# TODO: Add threading support".to_string()),
                _ => Some(format!("# TODO: Map {cmake_name} to Bazel")),
            };
        }
    }

    // Handle common system libraries (built-in, no explicit dep needed)
    match cmake_name {
        "pthread" | "Threads::Threads" | "m" | "dl" => None,
        _ => Some(format!("# TODO: Map '{cmake_name}' to Bazel dependency")),
    }
}

/// Normalize CMake include directory paths for Bazel.
fn normalize_include_dir(dir: &str) -> String {
    // Remove CMake variable references like ${CMAKE_CURRENT_SOURCE_DIR}
    if dir.starts_with("${") {
        if let Some(end) = dir.find('}') {
            let rest = &dir[end + 1..];
            return rest.trim_start_matches('/').to_string();
        }
    }
    dir.to_string()
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
        let content = generate_module_bazel(&project);

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
        assert!(content.contains("-O2"));
        assert!(content.contains("-g"));
    }

    #[test]
    fn test_generate_module_bazel_format() {
        let project = test_project();
        let content = generate_module_bazel(&project);

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

        let module = generate_module_bazel(&project);
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
