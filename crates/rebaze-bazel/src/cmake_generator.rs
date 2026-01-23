//! Bazel file generation from CMake projects.

use std::fmt::Write;

use rebaze_cmake::{CMakeProject, Executable, Library, LibraryKind};

/// Generate a MODULE.bazel file for a CMake project.
#[must_use]
pub fn generate_module_bazel(project: &CMakeProject) -> String {
    let mut content = String::new();
    let module_name = project.name.replace('-', "_");

    let _ = write!(
        content,
        r#""""Bazel module for {name} - migrated from CMake by rebaze."""

module(
    name = "{module_name}",
    version = "0.1.0",
)

# C/C++ toolchain
bazel_dep(name = "rules_cc", version = "0.1.1")

"#,
        name = project.name
    );

    // Add platform support if needed
    if !project.packages.is_empty() {
        content.push_str("# Platform and dependency management\n");
        content.push_str("bazel_dep(name = \"platforms\", version = \"0.0.11\")\n\n");
    }

    content
}

/// Generate the root BUILD.bazel file.
#[must_use]
pub fn generate_root_build(project: &CMakeProject) -> String {
    let mut content = String::new();
    let name = &project.name;

    let _ = writeln!(
        content,
        "# BUILD file for {name} - migrated from CMake by rebaze\n"
    );

    content.push_str("load(\"@rules_cc//cc:defs.bzl\", \"cc_binary\", \"cc_library\")\n\n");

    // Generate package visibility
    content.push_str("package(default_visibility = [\"//visibility:public\"])\n\n");

    // Generate libraries first (they may be dependencies of executables)
    for lib in &project.libraries {
        let _ = write!(content, "{}", generate_cc_library(lib));
        content.push('\n');
    }

    // Generate executables
    for exe in &project.executables {
        let _ = write!(content, "{}", generate_cc_binary(exe, &project.libraries));
        content.push('\n');
    }

    content
}

/// Generate a cc_library rule.
fn generate_cc_library(lib: &Library) -> String {
    let mut content = String::new();
    let target_name = lib.name.replace('-', "_");

    let _ = writeln!(content, "cc_library(");
    let _ = writeln!(content, "    name = \"{target_name}\",");

    // Determine if this is header-only
    let is_header_only = lib.kind == LibraryKind::Interface || lib.sources.is_empty();

    if is_header_only {
        // Header-only library
        let _ = writeln!(
            content,
            "    hdrs = glob([\"include/**/*.h\", \"include/**/*.hpp\"]),"
        );
        let _ = writeln!(content, "    includes = [\"include\"],");
    } else {
        // Regular library with sources
        let _ = writeln!(content, "    srcs = [");
        for src in &lib.sources {
            let _ = writeln!(content, "        \"{src}\",");
        }
        content.push_str("    ],\n");

        // Add headers
        let _ = writeln!(
            content,
            "    hdrs = glob([\"include/**/*.h\", \"include/**/*.hpp\"]),"
        );

        if lib.include_directories.is_empty() {
            let _ = writeln!(content, "    includes = [\"include\"],");
        } else {
            let _ = writeln!(content, "    includes = [");
            for dir in &lib.include_directories {
                // Convert CMake paths to Bazel-friendly paths
                let dir = normalize_include_dir(dir);
                let _ = writeln!(content, "        \"{dir}\",");
            }
            content.push_str("    ],\n");
        }
    }

    // Add link dependencies
    if !lib.link_libraries.is_empty() {
        let _ = writeln!(content, "    deps = [");
        for dep in &lib.link_libraries {
            if let Some(bazel_dep) = map_cmake_dependency(dep) {
                let _ = writeln!(content, "        \"{bazel_dep}\",");
            }
        }
        content.push_str("    ],\n");
    }

    // Add linkopts for specific library types
    if lib.kind == LibraryKind::Shared {
        let _ = writeln!(content, "    linkshared = True,");
    }

    content.push_str(")\n");
    content
}

/// Generate a cc_binary rule.
fn generate_cc_binary(exe: &Executable, libs: &[Library]) -> String {
    let mut content = String::new();
    let target_name = exe.name.replace('-', "_");

    let _ = writeln!(content, "cc_binary(");
    let _ = writeln!(content, "    name = \"{target_name}\",");

    let _ = writeln!(content, "    srcs = [");
    for src in &exe.sources {
        let _ = writeln!(content, "        \"{src}\",");
    }
    content.push_str("    ],\n");

    // Collect dependencies
    let mut deps = Vec::new();

    // Add internal library dependencies
    for link_lib in &exe.link_libraries {
        // Check if it's an internal library
        if libs.iter().any(|l| l.name == *link_lib) {
            deps.push(format!(":{}", link_lib.replace('-', "_")));
        } else if let Some(bazel_dep) = map_cmake_dependency(link_lib) {
            deps.push(bazel_dep);
        }
    }

    if !deps.is_empty() {
        let _ = writeln!(content, "    deps = [");
        for dep in &deps {
            let _ = writeln!(content, "        \"{dep}\",");
        }
        content.push_str("    ],\n");
    }

    content.push_str(")\n");
    content
}

/// Map CMake package/library names to Bazel dependencies.
fn map_cmake_dependency(cmake_name: &str) -> Option<String> {
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
            }],
            libraries: vec![Library {
                name: "mylib".to_string(),
                kind: LibraryKind::Static,
                sources: vec!["src/lib.cpp".to_string()],
                link_libraries: vec![],
                include_directories: vec!["include".to_string()],
            }],
            packages: vec![Package {
                name: "Boost".to_string(),
                version: Some("1.70".to_string()),
                required: true,
                components: vec!["system".to_string()],
            }],
            subdirectories: vec![],
        }
    }

    #[test]
    fn test_generate_module_bazel() {
        let project = test_project();
        let content = generate_module_bazel(&project);

        assert!(content.contains("module("));
        assert!(content.contains("test_project"));
        assert!(content.contains("rules_cc"));
    }

    #[test]
    fn test_generate_root_build() {
        let project = test_project();
        let content = generate_root_build(&project);

        assert!(content.contains("cc_library("));
        assert!(content.contains("cc_binary("));
        assert!(content.contains("mylib"));
        assert!(content.contains("myapp"));
    }
}
