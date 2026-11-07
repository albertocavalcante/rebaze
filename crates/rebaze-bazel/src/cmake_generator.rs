//! Bazel file generation from CMake projects.

use std::collections::BTreeSet;

use rebaze_cmake::{CMakeProject, Executable, Library, LibraryKind};

use crate::GenerationResult;
use crate::config::MigrationConfig;
use crate::dependency_plan::DependencyPlan;
use crate::filters::{filter_copts, filter_defines, filter_includes, filter_sources};
use crate::native_links::NativeLinkPlan;
use crate::starlark::{
    BazelDep, Call, CcBinary, CcLibrary, CcTest, File, Glob, Identifier, Load, Module, Package,
    Reference, RenderResult, SrcsWithHdrs, UseExtension, UseRepo,
};

/// BCR resolution result for a project.
#[derive(Debug, Default)]
struct BcrResolution {
    /// BCR dependencies to add to MODULE.bazel
    deps: Vec<BazelDep>,
    /// Packages that couldn't be resolved (need fallback)
    unresolved: Vec<String>,
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
        .chain(
            project
                .libraries
                .iter()
                .flat_map(|l| l.link_libraries.iter()),
        );

    for lib in all_link_libs {
        if project.pkg_config_modules.iter().any(|request| {
            lib == &format!("PkgConfig::{}", request.prefix)
                || request.packages.iter().any(|package| {
                    let name = package
                        .split(['<', '>', '='])
                        .next()
                        .unwrap_or_default()
                        .trim();
                    lib == &format!("-l{name}")
                })
        }) {
            continue;
        }
        let lib_lower = lib.to_lowercase();
        if lib_lower.contains("gtest")
            || lib_lower.contains("gmock")
            || lib_lower.contains("googletest")
        {
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

fn push_module_header(
    parts: &mut File,
    project: &CMakeProject,
    config: &MigrationConfig,
) -> RenderResult<()> {
    parts.docstring(&format!(
        "Bazel module for {} - migrated from CMake by rebaze.",
        project.name
    ))?;
    parts.statement(&Module {
        name: project.name.to_lowercase().replace('-', "_"),
        version: config.build.module_version.clone(),
    })
}

pub fn base_dependencies(
    project: &CMakeProject,
    config: &MigrationConfig,
) -> Vec<(String, String)> {
    let mut deps = vec![("rules_cc".into(), config.versions.rules_cc.clone())];
    let detected = detect_external_deps(project);
    if detected.googletest {
        deps.push(("googletest".into(), config.versions.googletest.clone()));
    }
    if detected.benchmark {
        deps.push((
            "google_benchmark".into(),
            config.versions.google_benchmark.clone(),
        ));
    }
    if !project.packages.is_empty() {
        deps.push(("platforms".into(), config.versions.platforms.clone()));
    }
    // Explicit non-pkg CMake mappings retain their existing contract.
    if config.strategy.default == "bcr" {
        deps.extend(
            resolve_bcr_deps(project, config)
                .deps
                .into_iter()
                .map(|dep| (dep.name, dep.version)),
        );
    }
    deps
}

/// Collect all package names from the CMake project that need resolution.
fn collect_all_packages(project: &CMakeProject) -> Vec<String> {
    let mut packages = BTreeSet::new();

    // Add CMake find_package names
    for pkg in &project.packages {
        packages.insert(pkg.name.clone());
        // Also add components for packages like Boost
        for comp in &pkg.components {
            packages.insert(format!("{}::{}", pkg.name, comp));
        }
    }

    // Add link libraries that look like package references
    for lib in &project.libraries {
        for link_lib in &lib.link_libraries {
            if link_lib.contains("::") {
                // CMake imported target like Boost::filesystem
                packages.insert(link_lib.clone());
            }
        }
    }

    for exe in &project.executables {
        for link_lib in &exe.link_libraries {
            if link_lib.contains("::") {
                packages.insert(link_lib.clone());
            }
        }
    }

    packages.into_iter().collect()
}

/// Resolve CMake packages to BCR modules.
fn resolve_bcr_deps(project: &CMakeProject, config: &MigrationConfig) -> BcrResolution {
    let package_names = collect_all_packages(project);
    let bcr_result = rebaze_bcr::resolve_cmake_deps(&package_names);

    let mut resolution = BcrResolution::default();

    for dep in bcr_result.resolved {
        // Check if this package should be skipped
        if config
            .strategy
            .bcr
            .skip_packages
            .contains(&dep.original_name)
        {
            resolution.unresolved.push(dep.original_name);
            continue;
        }

        // Apply module overrides
        let module_name = config
            .strategy
            .bcr
            .module_overrides
            .get(&dep.module)
            .cloned()
            .unwrap_or(dep.module);

        // Apply version pins
        let version = config
            .strategy
            .bcr
            .version_pins
            .get(&module_name)
            .cloned()
            .unwrap_or(dep.version);

        resolution.deps.push(BazelDep {
            name: module_name,
            version,
        });
    }

    for unresolved in bcr_result.unresolved {
        resolution.unresolved.push(unresolved.name);
    }

    // Deduplicate deps by module name (keep first occurrence)
    let mut seen = BTreeSet::new();
    resolution.deps.retain(|dep| seen.insert(dep.name.clone()));

    resolution
}

/// Generate MODULE.bazel from the same plan used by aliases and source targets.
pub fn generate_module_bazel(
    project: &CMakeProject,
    config: &MigrationConfig,
    plan: &DependencyPlan,
) -> RenderResult<String> {
    let mut file = File::new();
    push_module_header(&mut file, project, config)?;
    for (name, version) in &plan.modules {
        file.statement(&BazelDep {
            name: name.clone(),
            version: version.clone(),
        })?;
    }
    if !plan.systems.is_empty() {
        file.comment("System wrappers use the observed host installation (non-hermetic).");
        file.assign(
            &Identifier::new("system_deps")?,
            &UseExtension("//third_party:system_deps.bzl", "system_deps"),
        )?;
        file.statement(&UseRepo {
            extension: Reference::new("system_deps")?,
            repositories: plan
                .systems
                .iter()
                .map(|package| format!("system_{}", crate::discovery::target_name(&package.prefix)))
                .collect(),
        })?;
    }
    if !plan.sources.is_empty() {
        file.assign(
            &Identifier::new("source_deps")?,
            &UseExtension("//third_party:source.bzl", "source_deps"),
        )?;
        file.statement(&UseRepo {
            extension: Reference::new("source_deps")?,
            repositories: plan
                .sources
                .keys()
                .map(|name| format!("{name}_src"))
                .collect(),
        })?;
    }
    file.render()
}

/// Generate the root BUILD.bazel, including configured custom rule names.
pub fn generate_root_build(
    project: &CMakeProject,
    config: &MigrationConfig,
    links: &NativeLinkPlan,
    tests: &[crate::cmake_tests::PlannedTest<'_>],
) -> GenerationResult<String> {
    let mut file = File::new();
    file.comment(&format!(
        "BUILD file for {} - migrated from CMake by rebaze",
        project.name
    ));
    file.comment("Review generated include paths and glob patterns (allow_empty=True).");
    file.load(Load {
        bzl: "@rules_cc//cc:defs.bzl".into(),
        items: ["cc_binary".into(), "cc_library".into(), "cc_test".into()].into(),
    })?;
    for load in &config.build.rules.extra_loads {
        file.load(Load {
            bzl: load.bzl.clone(),
            items: load.items.iter().cloned().collect(),
        })?;
    }
    for package in &project.pkg_config_modules {
        file.comment(&format!(
            "//third_party:{} -> {}",
            crate::discovery::target_name(&package.prefix),
            package.packages.join(", ")
        ));
    }
    file.statement(&Package {
        default_visibility: vec![config.build.default_visibility.clone()],
    })?;
    for library in &project.libraries {
        let rule = build_cc_library(library, links)?;
        file.statement(&Call::new(&config.build.rules.library_rule, &rule)?)?;
    }
    for executable in &project.executables {
        let rule = build_cc_binary(executable, links)?;
        file.statement(&Call::new(&config.build.rules.binary_rule, &rule)?)?;
    }
    for test in tests {
        let binary = build_cc_binary(test.executable, links)?;
        file.statement(&Call::new(
            &config.build.rules.test_rule,
            &CcTest {
                name: test.name.clone(),
                binary: &binary,
                args: &test.registration.args,
            },
        )?)?;
    }
    Ok(file.render()?)
}

/// Build a CcLibrary struct from a CMake Library.
fn build_cc_library(lib: &Library, links: &NativeLinkPlan) -> GenerationResult<CcLibrary> {
    let target = links.target(&lib.name)?;

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

    Ok(CcLibrary {
        name: target.name.clone(),
        srcs,
        hdrs,
        includes,
        deps: target.dependencies.clone(),
        defines: filter_defines(&lib.compile_definitions),
        copts: filter_copts(&lib.compile_options),
        linkopts: Vec::new(),
        // For shared libraries, set linkstatic = False (default is True)
        linkstatic: if lib.kind == LibraryKind::Shared {
            Some(false)
        } else {
            None
        },
    })
}

/// Build a CcBinary struct from a CMake Executable.
fn build_cc_binary(exe: &Executable, links: &NativeLinkPlan) -> GenerationResult<CcBinary> {
    let target = links.target(&exe.name)?;

    // Filter sources first
    let filtered_sources = filter_sources(&exe.sources);

    // Start with explicit include directories (filtered)
    let mut includes_set: std::collections::BTreeSet<String> =
        filter_includes(&exe.include_directories)
            .into_iter()
            .collect();

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

    Ok(CcBinary {
        name: target.name.clone(),
        srcs: SrcsWithHdrs {
            files: filtered_sources,
            hdrs_glob: if hdrs_glob.is_empty() {
                None
            } else {
                Some(hdrs_glob)
            },
        },
        includes,
        deps: target.dependencies.clone(),
        defines: filter_defines(&exe.compile_definitions),
        copts: filter_copts(&exe.compile_options),
    })
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
#[allow(clippy::unwrap_used)]
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
            aliases: Vec::new(),
            testing: rebaze_cmake::Testing::default(),
        }
    }

    #[test]
    fn test_generate_module_bazel() {
        let project = test_project();
        let config = crate::config::MigrationConfig::default();
        let content = generate_module_bazel(
            &project,
            &config,
            &DependencyPlan::new(&project, &config).unwrap(),
        )
        .unwrap();

        assert!(content.contains("module("));
        assert!(content.contains("test_project"));
        assert!(content.contains("rules_cc"));
        assert!(content.contains("platforms"));
    }

    #[test]
    fn test_generate_root_build() {
        let project = test_project();
        let config = crate::config::MigrationConfig::default();
        let content = generate_root_build(
            &project,
            &config,
            &crate::plan_cmake(&project, &config).unwrap().links,
            &[],
        )
        .unwrap();

        assert!(content.contains("cc_library("));
        assert!(content.contains("cc_binary("));
        assert!(content.contains("mylib"));
        assert!(content.contains("myapp"));
        assert!(content.contains("LIB_DEF"));
        assert!(content.contains("APP_DEF"));
        // Optimization and debug flags are filtered out (Bazel handles these)
        assert!(
            !content.contains("-O2"),
            "optimization flags should be filtered"
        );
        assert!(
            !content.contains("\"-g\""),
            "debug info flags should be filtered"
        );
    }

    #[test]
    fn test_generate_module_bazel_format() {
        let project = test_project();
        let config = crate::config::MigrationConfig::default();
        let content = generate_module_bazel(
            &project,
            &config,
            &DependencyPlan::new(&project, &config).unwrap(),
        )
        .unwrap();

        // Check docstring is present
        assert!(content.contains("\"Bazel module for"));

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
        let config = crate::config::MigrationConfig::default();
        let content = generate_root_build(
            &project,
            &config,
            &crate::plan_cmake(&project, &config).unwrap().links,
            &[],
        )
        .unwrap();

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

        let module = generate_module_bazel(
            &project,
            &config,
            &DependencyPlan::new(&project, &config).unwrap(),
        )
        .unwrap();
        let build = generate_root_build(
            &project,
            &config,
            &crate::plan_cmake(&project, &config).unwrap().links,
            &[],
        )
        .unwrap();

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
