//! CMake project model extraction.
//!
//! Interprets parsed CMake commands to build a project model
//! suitable for Bazel migration.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::ast::{Argument, CMakeFile, Command};
use crate::eval::EvalContext;

/// Error type for project extraction.
#[derive(Debug, thiserror::Error)]
pub enum ExtractError {
    /// Failed to read a CMakeLists.txt file.
    #[error("Failed to read {path}: {source}")]
    ReadFile {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// Failed to canonicalize a path.
    #[error("Failed to canonicalize path {path}: {source}")]
    Canonicalize {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// CMakeLists.txt not found.
    #[error("CMakeLists.txt not found at {0}")]
    NotFound(PathBuf),

    /// Parse error in CMakeLists.txt.
    #[error("Parse error in {path}: {message}")]
    Parse { path: PathBuf, message: String },
}

/// A parsed CMake project.
#[derive(Debug, Clone, Default)]
pub struct CMakeProject {
    /// Project name from project() command.
    pub name: String,
    /// Project version if specified.
    pub version: Option<String>,
    /// Languages used (C, CXX, etc.).
    pub languages: Vec<String>,
    /// Minimum CMake version required.
    pub cmake_minimum_version: Option<String>,
    /// Root path of the project.
    pub path: PathBuf,
    /// Executable targets.
    pub executables: Vec<Executable>,
    /// Library targets.
    pub libraries: Vec<Library>,
    /// External package dependencies.
    pub packages: Vec<Package>,
    /// pkg-config modules (from pkg_check_modules).
    pub pkg_config_modules: Vec<PkgConfigModule>,
    /// Subdirectories (add_subdirectory calls).
    pub subdirectories: Vec<String>,
    /// Global include directories (from include_directories() commands).
    pub global_include_directories: Vec<String>,
    /// C++ standard version detected (e.g., "11", "14", "17", "20", "23").
    /// Extracted from CMAKE_CXX_STANDARD, target_compile_features, or compile flags.
    pub cxx_standard: Option<String>,
    /// C standard version detected (e.g., "99", "11", "17", "23").
    pub c_standard: Option<String>,
}

/// An executable target.
#[derive(Debug, Clone)]
pub struct Executable {
    pub name: String,
    pub sources: Vec<String>,
    pub link_libraries: Vec<String>,
    pub include_directories: Vec<String>,
    pub compile_definitions: Vec<String>,
    pub compile_options: Vec<String>,
}

/// A library target.
#[derive(Debug, Clone)]
pub struct Library {
    pub name: String,
    pub kind: LibraryKind,
    pub sources: Vec<String>,
    pub link_libraries: Vec<String>,
    pub include_directories: Vec<String>,
    pub compile_definitions: Vec<String>,
    pub compile_options: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LibraryKind {
    Static,
    Shared,
    Module,
    Object,
    Interface,
    Unknown,
}

/// An external package dependency.
#[derive(Debug, Clone)]
pub struct Package {
    pub name: String,
    pub version: Option<String>,
    pub required: bool,
    pub components: Vec<String>,
}

/// A pkg-config module dependency.
#[derive(Debug, Clone)]
pub struct PkgConfigModule {
    /// The CMake variable prefix (e.g., "GLIB")
    pub prefix: String,
    /// The pkg-config package names (e.g., `["glib-2.0"]`)
    pub packages: Vec<String>,
    /// Whether this package is required
    pub required: bool,
}

/// Extract project information from a parsed CMake file.
pub fn extract_project(file: &CMakeFile, path: PathBuf) -> CMakeProject {
    let mut ctx = EvalContext::new();
    extract_project_with_context(file, path, &mut ctx)
}

/// Extract project information from a parsed CMake file with an existing context.
pub fn extract_project_with_context(
    file: &CMakeFile,
    path: PathBuf,
    ctx: &mut EvalContext,
) -> CMakeProject {
    let mut project = CMakeProject {
        path,
        ..Default::default()
    };

    // Evaluate set() and list() commands to populate variables
    crate::eval::evaluate(file, ctx);

    // Extract C/C++ standards from variables (CMAKE_CXX_STANDARD, CMAKE_C_STANDARD)
    if let Some(values) = ctx.get("CMAKE_CXX_STANDARD")
        && let Some(std) = values.first()
    {
        project.cxx_standard = Some(std.clone());
    }
    if let Some(values) = ctx.get("CMAKE_C_STANDARD") && let Some(std) = values.first() {
        project.c_standard = Some(std.clone());
    }

    // First pass: extract basic project info and targets
    for cmd in &file.commands {
        match cmd.name.as_str() {
            "cmake_minimum_required" => extract_cmake_version(cmd, &mut project),
            "project" => extract_project_info(cmd, &mut project),
            "add_executable" => extract_executable(cmd, &mut project, ctx),
            "add_library" => extract_library(cmd, &mut project, ctx),
            "find_package" => extract_package(cmd, &mut project),
            "pkg_check_modules" => extract_pkg_config(cmd, &mut project, ctx),
            "add_subdirectory" => extract_subdirectory(cmd, &mut project),
            "include_directories" => extract_global_includes(cmd, &mut project, ctx),
            "target_compile_features" => extract_compile_features(cmd, &mut project),
            _ => {}
        }
    }

    // Second pass: attach target properties
    for cmd in &file.commands {
        match cmd.name.as_str() {
            "target_link_libraries" => apply_link_libraries(cmd, &mut project, ctx),
            "target_include_directories" => apply_include_directories(cmd, &mut project, ctx),
            "target_compile_definitions" => apply_compile_definitions(cmd, &mut project, ctx),
            "target_compile_options" => apply_compile_options(cmd, &mut project, ctx),
            "target_sources" => apply_target_sources(cmd, &mut project, ctx),
            _ => {}
        }
    }

    // Third pass: apply global include directories to all targets
    if !project.global_include_directories.is_empty() {
        let global_dirs = project.global_include_directories.clone();
        for exe in &mut project.executables {
            for dir in &global_dirs {
                if !exe.include_directories.contains(dir) {
                    exe.include_directories.push(dir.clone());
                }
            }
        }
        for lib in &mut project.libraries {
            for dir in &global_dirs {
                if !lib.include_directories.contains(dir) {
                    lib.include_directories.push(dir.clone());
                }
            }
        }
    }

    project
}

/// Extract a CMake project from a directory path, recursively parsing subdirectories.
///
/// This function reads `{root}/CMakeLists.txt` and recursively parses any
/// subdirectories referenced via `add_subdirectory()` commands, merging all
/// targets into a single project.
///
/// # Errors
/// Returns an error if the CMakeLists.txt cannot be read or parsed.
pub fn extract_project_from_path(root: &Path) -> Result<CMakeProject, ExtractError> {
    let mut visited = HashSet::new();
    extract_recursive(root, &mut visited)
}

/// Recursively extract project information from a directory.
fn extract_recursive(
    dir: &Path,
    visited: &mut HashSet<PathBuf>,
) -> Result<CMakeProject, ExtractError> {
    // Canonicalize to avoid visiting the same directory twice via symlinks
    let canonical = dir.canonicalize().map_err(|e| ExtractError::Canonicalize {
        path: dir.to_path_buf(),
        source: e,
    })?;

    // Skip if already visited (prevents infinite loops)
    if !visited.insert(canonical) {
        tracing::debug!("Skipping already visited directory: {}", dir.display());
        return Ok(CMakeProject::default());
    }

    let cmake_path = dir.join("CMakeLists.txt");
    if !cmake_path.exists() {
        return Err(ExtractError::NotFound(cmake_path));
    }

    tracing::debug!("Parsing CMakeLists.txt at {}", cmake_path.display());

    let source = std::fs::read_to_string(&cmake_path).map_err(|e| ExtractError::ReadFile {
        path: cmake_path.clone(),
        source: e,
    })?;

    let (file, errors) = crate::parser::parse(&source);

    if !errors.is_empty() {
        let error_msgs: Vec<String> = errors
            .iter()
            .map(|e| format!("at {}: {e}", e.span().start))
            .collect();
        return Err(ExtractError::Parse {
            path: cmake_path,
            message: error_msgs.join("; "),
        });
    }

    let file = file.ok_or_else(|| ExtractError::Parse {
        path: cmake_path.clone(),
        message: "Parse returned no file".to_string(),
    })?;

    // Extract the project from the current file
    let mut project = extract_project(&file, dir.to_path_buf());

    // Recursively process subdirectories
    let subdirs = std::mem::take(&mut project.subdirectories);
    for subdir_name in &subdirs {
        let subdir_path = dir.join(subdir_name);

        // Skip if the subdirectory doesn't exist (might be conditionally added)
        if !subdir_path.exists() {
            tracing::debug!(
                "Subdirectory {} does not exist, skipping",
                subdir_path.display()
            );
            continue;
        }

        match extract_recursive(&subdir_path, visited) {
            Ok(subproject) => {
                // Merge targets from subdirectory into main project,
                // prefixing source and include paths with the subdirectory name
                for mut exe in subproject.executables {
                    exe.sources = exe
                        .sources
                        .into_iter()
                        .map(|s| format!("{subdir_name}/{s}"))
                        .collect();
                    exe.include_directories = exe
                        .include_directories
                        .into_iter()
                        .map(|d| prefix_path(subdir_name, &d))
                        .collect();
                    project.executables.push(exe);
                }
                for mut lib in subproject.libraries {
                    lib.sources = lib
                        .sources
                        .into_iter()
                        .map(|s| format!("{subdir_name}/{s}"))
                        .collect();
                    lib.include_directories = lib
                        .include_directories
                        .into_iter()
                        .map(|d| prefix_path(subdir_name, &d))
                        .collect();
                    project.libraries.push(lib);
                }
                project.packages.extend(subproject.packages);
                project.pkg_config_modules.extend(subproject.pkg_config_modules);
                // Note: We don't merge project name/version from subdirectories
            }
            Err(ExtractError::NotFound(_)) => {
                // Subdirectory exists but has no CMakeLists.txt - this is fine
                tracing::debug!(
                    "No CMakeLists.txt in subdirectory {}, skipping",
                    subdir_path.display()
                );
            }
            Err(e) => {
                // Propagate other errors
                return Err(e);
            }
        }
    }

    // Restore subdirectories list (for reference)
    project.subdirectories = subdirs;

    Ok(project)
}

fn extract_cmake_version(cmd: &Command, project: &mut CMakeProject) {
    // cmake_minimum_required(VERSION x.y.z)
    for i in 0..cmd.arguments.len() {
        let Some(arg) = cmd.arguments[i].as_literal() else {
            continue;
        };
        if arg.eq_ignore_ascii_case("VERSION")
            && let Some(version) = cmd
                .arguments
                .get(i + 1)
                .and_then(Argument::as_literal)
        {
            project.cmake_minimum_version = Some(version.to_string());
            return;
        }
    }
}

fn extract_project_info(cmd: &Command, project: &mut CMakeProject) {
    // project(name [lang1 lang2...] | [VERSION x.y.z] [LANGUAGES lang1 lang2...])
    // CMake supports both positional languages (after name, before keywords) and explicit LANGUAGES keyword
    let name = cmd.arguments.first().and_then(Argument::as_literal);
    if let Some(name) = name {
        project.name = name.to_string();
    } else {
        tracing::debug!("Skipping non-literal project name");
    }

    // First, collect any positional language arguments (right after name, before any keyword)
    let mut positional_languages = Vec::new();
    let mut i = 1;
    while i < cmd.arguments.len() {
        let Some(arg) = cmd.arguments[i].as_literal() else {
            i += 1;
            continue;
        };
        // If we hit a keyword, stop collecting positional languages
        if is_project_keyword(arg) {
            break;
        }
        // Check if this looks like a language (C, CXX, Fortran, etc.)
        if is_language(arg) {
            positional_languages.push(arg.to_string());
        }
        i += 1;
    }

    // Now parse keyword arguments
    while i < cmd.arguments.len() {
        let Some(arg) = cmd.arguments[i].as_literal() else {
            i += 1;
            continue;
        };
        match arg.to_uppercase().as_str() {
            "VERSION" => {
                if let Some(ver) = cmd
                    .arguments
                    .get(i + 1)
                    .and_then(Argument::as_literal)
                {
                    project.version = Some(ver.to_string());
                }
                i += 2;
            }
            "LANGUAGES" => {
                i += 1;
                while i < cmd.arguments.len() {
                    let Some(lang) = cmd.arguments[i].as_literal() else {
                        i += 1;
                        continue;
                    };
                    if is_project_keyword(lang) {
                        break;
                    }
                    project.languages.push(lang.to_string());
                    i += 1;
                }
            }
            _ => i += 1,
        }
    }

    // Use positional languages if no LANGUAGES keyword was found
    if project.languages.is_empty() && !positional_languages.is_empty() {
        project.languages = positional_languages;
    }

    // Default to C and CXX if no languages specified at all
    if project.languages.is_empty() {
        project.languages = vec!["C".to_string(), "CXX".to_string()];
    }
}

/// Check if the argument is a project() keyword
fn is_project_keyword(arg: &str) -> bool {
    matches!(
        arg.to_uppercase().as_str(),
        "VERSION" | "DESCRIPTION" | "HOMEPAGE_URL" | "LANGUAGES"
    )
}

/// Check if the argument looks like a CMake language
fn is_language(arg: &str) -> bool {
    matches!(
        arg.to_uppercase().as_str(),
        "C" | "CXX" | "CUDA" | "OBJC" | "OBJCXX" | "Fortran" | "HIP" | "ISPC" | "ASM" | "NONE"
    )
}

fn extract_executable(cmd: &Command, project: &mut CMakeProject, ctx: &EvalContext) {
    // add_executable(name [WIN32] [MACOSX_BUNDLE] source1 source2...)
    let name = match cmd.arguments.first().and_then(Argument::as_literal) {
        Some(name) => name.to_string(),
        None => {
            // Try expanding via context
            if let Some(first_arg) = cmd.arguments.first() {
                let mut expanded = ctx.expand_argument(first_arg);
                if expanded.len() == 1 {
                    expanded.remove(0)
                } else {
                    tracing::debug!("Skipping add_executable with non-literal target name");
                    return;
                }
            } else {
                tracing::debug!("Skipping add_executable with no arguments");
                return;
            }
        }
    };

    let mut sources = Vec::new();
    for arg in cmd.arguments.iter().skip(1) {
        // Try to get as literal first
        if let Some(lit) = arg.as_literal() {
            if matches!(
                lit.to_uppercase().as_str(),
                "WIN32" | "MACOSX_BUNDLE" | "EXCLUDE_FROM_ALL"
            ) {
                continue;
            }
            sources.push(lit.to_string());
        } else {
            // Expand variable references
            let expanded = ctx.expand_argument(arg);
            for val in expanded {
                if matches!(
                    val.to_uppercase().as_str(),
                    "WIN32" | "MACOSX_BUNDLE" | "EXCLUDE_FROM_ALL"
                ) {
                    continue;
                }
                sources.push(val);
            }
        }
    }

    project.executables.push(Executable {
        name,
        sources,
        link_libraries: Vec::new(),
        include_directories: Vec::new(),
        compile_definitions: Vec::new(),
        compile_options: Vec::new(),
    });
}

fn extract_library(cmd: &Command, project: &mut CMakeProject, ctx: &EvalContext) {
    // add_library(name [STATIC|SHARED|MODULE|OBJECT|INTERFACE] source1 source2...)
    let name = match cmd.arguments.first().and_then(Argument::as_literal) {
        Some(name) => name.to_string(),
        None => {
            // Try expanding via context
            if let Some(first_arg) = cmd.arguments.first() {
                let mut expanded = ctx.expand_argument(first_arg);
                if expanded.len() == 1 {
                    expanded.remove(0)
                } else {
                    tracing::debug!("Skipping add_library with non-literal target name");
                    return;
                }
            } else {
                tracing::debug!("Skipping add_library with no arguments");
                return;
            }
        }
    };

    let mut kind = LibraryKind::Unknown;
    let mut sources = Vec::new();
    let mut skip_target = false;

    for arg in cmd.arguments.iter().skip(1) {
        // Try to get as literal first
        if let Some(lit) = arg.as_literal() {
            match lit.to_uppercase().as_str() {
                "STATIC" => kind = LibraryKind::Static,
                "SHARED" => kind = LibraryKind::Shared,
                "MODULE" => kind = LibraryKind::Module,
                "OBJECT" => kind = LibraryKind::Object,
                "INTERFACE" => kind = LibraryKind::Interface,
                "EXCLUDE_FROM_ALL" => {}
                "IMPORTED" | "ALIAS" => {
                    skip_target = true;
                    break;
                }
                _ => sources.push(lit.to_string()),
            }
        } else {
            // Expand variable references
            let expanded = ctx.expand_argument(arg);
            for val in expanded {
                match val.to_uppercase().as_str() {
                    "STATIC" => kind = LibraryKind::Static,
                    "SHARED" => kind = LibraryKind::Shared,
                    "MODULE" => kind = LibraryKind::Module,
                    "OBJECT" => kind = LibraryKind::Object,
                    "INTERFACE" => kind = LibraryKind::Interface,
                    "EXCLUDE_FROM_ALL" => {}
                    "IMPORTED" | "ALIAS" => {
                        skip_target = true;
                        break;
                    }
                    _ => sources.push(val),
                }
            }
            if skip_target {
                break;
            }
        }
    }

    if skip_target {
        tracing::debug!("Skipping imported or alias library target '{name}'");
        return;
    }

    project.libraries.push(Library {
        name,
        kind,
        sources,
        link_libraries: Vec::new(),
        include_directories: Vec::new(),
        compile_definitions: Vec::new(),
        compile_options: Vec::new(),
    });
}

fn extract_package(cmd: &Command, project: &mut CMakeProject) {
    // find_package(PackageName [version] [REQUIRED] [COMPONENTS comp1...])
    let name = if let Some(name) = cmd.arguments.first().and_then(Argument::as_literal) { name.to_string() } else {
        tracing::debug!("Skipping find_package with non-literal package name");
        return;
    };

    let mut version = None;
    let mut required = false;
    let mut components = Vec::new();
    let mut in_components = false;

    for arg in cmd.arguments.iter().skip(1) {
        let Some(lit) = arg.as_literal() else {
            continue;
        };
        let upper = lit.to_uppercase();
        match upper.as_str() {
            "REQUIRED" => required = true,
            "COMPONENTS" | "OPTIONAL_COMPONENTS" => in_components = true,
            "CONFIG" | "MODULE" | "NO_MODULE" | "QUIET" | "EXACT" => in_components = false,
            _ => {
                if in_components {
                    components.push(lit.to_string());
                } else if version.is_none() {
                    // First non-keyword arg after name might be version.
                    version = Some(lit.to_string());
                }
            }
        }
    }

    project.packages.push(Package {
        name,
        version,
        required,
        components,
    });
}

fn extract_subdirectory(cmd: &Command, project: &mut CMakeProject) {
    if let Some(dir) = cmd.arg_literal(0) {
        project.subdirectories.push(dir.to_string());
    }
}

fn extract_global_includes(cmd: &Command, project: &mut CMakeProject, ctx: &EvalContext) {
    // include_directories([AFTER|BEFORE] [SYSTEM] dir1 [dir2 ...])
    for arg in &cmd.arguments {
        // Try to get as literal first
        if let Some(lit) = arg.as_literal() {
            // Skip keywords
            if matches!(
                lit.to_uppercase().as_str(),
                "AFTER" | "BEFORE" | "SYSTEM"
            ) {
                continue;
            }
            let dir = normalize_include_path(lit);
            if !dir.is_empty() && !project.global_include_directories.contains(&dir) {
                project.global_include_directories.push(dir);
            }
        } else {
            // First try to normalize the raw argument with variable references preserved
            // This handles ${CMAKE_CURRENT_SOURCE_DIR} and similar patterns
            let raw = arg.to_string_with_vars();
            if !matches!(
                raw.to_uppercase().as_str(),
                "AFTER" | "BEFORE" | "SYSTEM"
            ) {
                let dir = normalize_include_path(&raw);
                if !dir.is_empty()
                    && !dir.starts_with("${")
                    && !project.global_include_directories.contains(&dir)
                {
                    project.global_include_directories.push(dir.clone());
                    continue;
                }
            }

            // Fall back to expanding variable references
            let expanded = ctx.expand_argument(arg);
            for val in expanded {
                if matches!(
                    val.to_uppercase().as_str(),
                    "AFTER" | "BEFORE" | "SYSTEM"
                ) {
                    continue;
                }
                let dir = normalize_include_path(&val);
                // Skip empty or unexpanded variable references
                if !dir.is_empty()
                    && !dir.starts_with("${")
                    && !project.global_include_directories.contains(&dir)
                {
                    project.global_include_directories.push(dir);
                }
            }
        }
    }
}

/// Extract compile features like cxx_std_17 from target_compile_features() commands.
fn extract_compile_features(cmd: &Command, project: &mut CMakeProject) {
    // target_compile_features(target PUBLIC|PRIVATE|INTERFACE feature1 feature2...)
    for arg in &cmd.arguments {
        if let Some(lit) = arg.as_literal() {
            // Look for cxx_std_XX patterns
            if let Some(std) = lit.strip_prefix("cxx_std_") {
                // Use the highest standard found
                let current = project.cxx_standard.as_deref().unwrap_or("0");
                if std > current {
                    project.cxx_standard = Some(std.to_string());
                }
            }
            // Look for c_std_XX patterns
            if let Some(std) = lit.strip_prefix("c_std_") {
                let current = project.c_standard.as_deref().unwrap_or("0");
                if std > current {
                    project.c_standard = Some(std.to_string());
                }
            }
        }
    }
}

/// Prefix a path with a subdirectory, handling "." specially.
fn prefix_path(subdir: &str, path: &str) -> String {
    if path == "." {
        subdir.to_string()
    } else if let Some(stripped) = path.strip_prefix("./") {
        format!("{subdir}/{stripped}")
    } else {
        format!("{subdir}/{path}")
    }
}

/// Normalize include path for Bazel compatibility.
fn normalize_include_path(path: &str) -> String {
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
    path.to_string()
}

fn extract_pkg_config(cmd: &Command, project: &mut CMakeProject, ctx: &mut EvalContext) {
    // pkg_check_modules(PREFIX [REQUIRED] [QUIET] pkg1 pkg2 ...)
    let args: Vec<&str> = cmd
        .arguments
        .iter()
        .filter_map(|a| a.as_literal())
        .collect();

    if args.is_empty() {
        return;
    }

    let prefix = args[0].to_string();
    let mut required = false;
    let mut packages = Vec::new();

    for arg in args.iter().skip(1) {
        match arg.to_uppercase().as_str() {
            "REQUIRED" => required = true,
            "QUIET" | "NO_CMAKE_PATH" | "NO_CMAKE_ENVIRONMENT_PATH" | "IMPORTED_TARGET" => {}
            _ => packages.push((*arg).to_string()),
        }
    }

    if !packages.is_empty() {
        // Set placeholder values so variable expansion works
        // e.g., ${GLIB_LIBRARIES} -> ["-lglib-2.0"]
        let lib_flags: Vec<String> = packages
            .iter()
            .map(|p| {
                // Strip version specifier (e.g., "glib-2.0>=2.44.0" -> "glib-2.0")
                let pkg_name = p.split(">=").next().unwrap_or(p);
                let pkg_name = pkg_name.split("<=").next().unwrap_or(pkg_name);
                let pkg_name = pkg_name.split('>').next().unwrap_or(pkg_name);
                let pkg_name = pkg_name.split('<').next().unwrap_or(pkg_name);
                let pkg_name = pkg_name.split('=').next().unwrap_or(pkg_name);
                format!("-l{pkg_name}")
            })
            .collect();

        ctx.set(&format!("{prefix}_LIBRARIES"), lib_flags);
        ctx.set(&format!("{prefix}_INCLUDE_DIRS"), vec![]);

        project.pkg_config_modules.push(PkgConfigModule {
            prefix,
            packages,
            required,
        });
    }
}

fn apply_link_libraries(cmd: &Command, project: &mut CMakeProject, ctx: &EvalContext) {
    // target_link_libraries(target [PUBLIC|PRIVATE|INTERFACE] lib1 lib2...)
    let target = match cmd.arguments.first().and_then(Argument::as_literal) {
        Some(target) => target.to_string(),
        None => return,
    };

    let mut libs = Vec::new();
    for arg in cmd.arguments.iter().skip(1) {
        if let Some(lit) = arg.as_literal() {
            if matches!(
                lit.to_uppercase().as_str(),
                "PUBLIC" | "PRIVATE" | "INTERFACE"
            ) {
                continue;
            }
            libs.push(lit.to_string());
        } else {
            // Expand variable references
            let expanded = ctx.expand_argument(arg);
            for val in expanded {
                if matches!(
                    val.to_uppercase().as_str(),
                    "PUBLIC" | "PRIVATE" | "INTERFACE"
                ) {
                    continue;
                }
                libs.push(val);
            }
        }
    }

    if libs.is_empty() {
        return;
    }

    // Find and update the target
    for exe in &mut project.executables {
        if exe.name == target {
            exe.link_libraries.extend(libs);
            return;
        }
    }
    for lib in &mut project.libraries {
        if lib.name == target {
            lib.link_libraries.extend(libs);
            return;
        }
    }
}

fn apply_include_directories(cmd: &Command, project: &mut CMakeProject, ctx: &EvalContext) {
    // target_include_directories(target [PUBLIC|PRIVATE|INTERFACE] dir1 dir2...)
    let target = match cmd.arguments.first().and_then(Argument::as_literal) {
        Some(target) => target.to_string(),
        None => return,
    };

    let mut dirs = Vec::new();
    for arg in cmd.arguments.iter().skip(1) {
        if let Some(lit) = arg.as_literal() {
            if matches!(
                lit.to_uppercase().as_str(),
                "PUBLIC" | "PRIVATE" | "INTERFACE" | "SYSTEM" | "BEFORE" | "AFTER"
            ) {
                continue;
            }
            dirs.push(lit.to_string());
        } else {
            // Expand variable references
            let expanded = ctx.expand_argument(arg);
            for val in expanded {
                if matches!(
                    val.to_uppercase().as_str(),
                    "PUBLIC" | "PRIVATE" | "INTERFACE" | "SYSTEM" | "BEFORE" | "AFTER"
                ) {
                    continue;
                }
                dirs.push(val);
            }
        }
    }

    if dirs.is_empty() {
        return;
    }

    for exe in &mut project.executables {
        if exe.name == target {
            exe.include_directories.extend(dirs);
            return;
        }
    }
    for lib in &mut project.libraries {
        if lib.name == target {
            lib.include_directories.extend(dirs);
            return;
        }
    }
}

fn apply_compile_definitions(cmd: &Command, project: &mut CMakeProject, ctx: &EvalContext) {
    // target_compile_definitions(target [PUBLIC|PRIVATE|INTERFACE] def1 def2...)
    let target = match cmd.arguments.first().and_then(Argument::as_literal) {
        Some(target) => target.to_string(),
        None => return,
    };

    let mut defs = Vec::new();
    for arg in cmd.arguments.iter().skip(1) {
        if let Some(lit) = arg.as_literal() {
            if matches!(
                lit.to_uppercase().as_str(),
                "PUBLIC" | "PRIVATE" | "INTERFACE"
            ) {
                continue;
            }

            let def = strip_define_prefix(lit);
            if !def.is_empty() {
                defs.push(def);
            }
        } else {
            // Expand variable references
            let expanded = ctx.expand_argument(arg);
            for val in expanded {
                if matches!(
                    val.to_uppercase().as_str(),
                    "PUBLIC" | "PRIVATE" | "INTERFACE"
                ) {
                    continue;
                }
                let def = strip_define_prefix(&val);
                if !def.is_empty() {
                    defs.push(def);
                }
            }
        }
    }

    if defs.is_empty() {
        return;
    }

    for exe in &mut project.executables {
        if exe.name == target {
            extend_unique(&mut exe.compile_definitions, defs);
            return;
        }
    }
    for lib in &mut project.libraries {
        if lib.name == target {
            extend_unique(&mut lib.compile_definitions, defs);
            return;
        }
    }
}

fn apply_compile_options(cmd: &Command, project: &mut CMakeProject, ctx: &EvalContext) {
    // target_compile_options(target [BEFORE] [SYSTEM] [PUBLIC|PRIVATE|INTERFACE] opt1 opt2...)
    let target = match cmd.arguments.first().and_then(Argument::as_literal) {
        Some(target) => target.to_string(),
        None => return,
    };

    let mut opts = Vec::new();
    let mut skip_next = false;
    for arg in cmd.arguments.iter().skip(1) {
        if skip_next {
            skip_next = false;
            continue;
        }
        if let Some(lit) = arg.as_literal() {
            if matches!(
                lit.to_uppercase().as_str(),
                "PUBLIC" | "PRIVATE" | "INTERFACE" | "SYSTEM" | "BEFORE"
            ) {
                continue;
            }
            if matches!(lit, "-I" | "-isystem" | "/I") {
                skip_next = true;
                continue;
            }
            if is_include_flag(lit) {
                continue;
            }
            if !lit.is_empty() {
                opts.push(lit.to_string());
            }
        } else {
            // Expand variable references
            let expanded = ctx.expand_argument(arg);
            for val in expanded {
                if matches!(
                    val.to_uppercase().as_str(),
                    "PUBLIC" | "PRIVATE" | "INTERFACE" | "SYSTEM" | "BEFORE"
                ) {
                    continue;
                }
                if matches!(val.as_str(), "-I" | "-isystem" | "/I") {
                    skip_next = true;
                    continue;
                }
                if is_include_flag(&val) {
                    continue;
                }
                if !val.is_empty() {
                    opts.push(val);
                }
            }
        }
    }

    if opts.is_empty() {
        return;
    }

    for exe in &mut project.executables {
        if exe.name == target {
            extend_unique(&mut exe.compile_options, opts);
            return;
        }
    }
    for lib in &mut project.libraries {
        if lib.name == target {
            extend_unique(&mut lib.compile_options, opts);
            return;
        }
    }
}

fn apply_target_sources(cmd: &Command, project: &mut CMakeProject, ctx: &EvalContext) {
    // target_sources(<target>
    //   <INTERFACE|PUBLIC|PRIVATE> [items1...]
    //   [<INTERFACE|PUBLIC|PRIVATE> [items2...] ...])
    // Also supports FILE_SET which we skip for now
    let target = match cmd.arguments.first().and_then(Argument::as_literal) {
        Some(target) => target.to_string(),
        None => return,
    };

    let mut sources = Vec::new();
    let mut skip_until_next_visibility = false;

    for arg in cmd.arguments.iter().skip(1) {
        if let Some(lit) = arg.as_literal() {
            let upper = lit.to_uppercase();
            // Reset skip state on visibility keywords
            if matches!(upper.as_str(), "PUBLIC" | "PRIVATE" | "INTERFACE") {
                skip_until_next_visibility = false;
                continue;
            }
            // FILE_SET introduces a block we should skip until next visibility keyword
            if upper == "FILE_SET" || upper == "TYPE" || upper == "BASE_DIRS" || upper == "FILES" {
                skip_until_next_visibility = true;
                continue;
            }
            if skip_until_next_visibility {
                continue;
            }
            // Skip generator expressions for now
            if lit.starts_with('$') {
                continue;
            }
            sources.push(lit.to_string());
        } else {
            // Expand variable references
            let expanded = ctx.expand_argument(arg);
            for val in expanded {
                let upper = val.to_uppercase();
                if matches!(upper.as_str(), "PUBLIC" | "PRIVATE" | "INTERFACE") {
                    skip_until_next_visibility = false;
                    continue;
                }
                if upper == "FILE_SET" || upper == "TYPE" || upper == "BASE_DIRS" || upper == "FILES" {
                    skip_until_next_visibility = true;
                    continue;
                }
                if skip_until_next_visibility {
                    continue;
                }
                if val.starts_with('$') {
                    continue;
                }
                sources.push(val);
            }
        }
    }

    if sources.is_empty() {
        return;
    }

    // Find and update the target
    for exe in &mut project.executables {
        if exe.name == target {
            exe.sources.extend(sources);
            return;
        }
    }
    for lib in &mut project.libraries {
        if lib.name == target {
            lib.sources.extend(sources);
            return;
        }
    }
}

fn strip_define_prefix(value: &str) -> String {
    if let Some(stripped) = value.strip_prefix("-D") {
        return stripped.to_string();
    }
    if let Some(stripped) = value.strip_prefix("/D") {
        return stripped.to_string();
    }
    value.to_string()
}

fn is_include_flag(value: &str) -> bool {
    matches!(value, "-I" | "-isystem" | "/I")
        || value.starts_with("-I")
        || value.starts_with("-isystem")
        || value.starts_with("/I")
}

fn extend_unique(target: &mut Vec<String>, items: Vec<String>) {
    for item in items {
        if !target.contains(&item) {
            target.push(item);
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::parser;

    fn parse_and_extract(src: &str) -> CMakeProject {
        let (file, errors) = parser::parse(src);
        assert!(errors.is_empty(), "Parse errors: {errors:?}");
        extract_project(&file.unwrap(), PathBuf::from("."))
    }

    #[test]
    fn test_project_extraction() {
        let project = parse_and_extract(
            "
            cmake_minimum_required(VERSION 3.20)
            project(myapp VERSION 1.0.0 LANGUAGES CXX)
        ",
        );

        assert_eq!(project.name, "myapp");
        assert_eq!(project.version, Some("1.0.0".to_string()));
        assert_eq!(project.languages, vec!["CXX"]);
        assert_eq!(project.cmake_minimum_version, Some("3.20".to_string()));
    }

    #[test]
    fn test_executable_extraction() {
        let project = parse_and_extract(
            "
            project(myapp)
            add_executable(myapp main.cpp util.cpp)
            target_link_libraries(myapp pthread)
        ",
        );

        assert_eq!(project.executables.len(), 1);
        let exe = &project.executables[0];
        assert_eq!(exe.name, "myapp");
        assert_eq!(exe.sources, vec!["main.cpp", "util.cpp"]);
        assert_eq!(exe.link_libraries, vec!["pthread"]);
    }

    #[test]
    fn test_library_extraction() {
        let project = parse_and_extract(
            "
            project(mylib)
            add_library(mylib STATIC lib.cpp)
            add_library(mylib_shared SHARED lib.cpp)
        ",
        );

        assert_eq!(project.libraries.len(), 2);
        assert_eq!(project.libraries[0].kind, LibraryKind::Static);
        assert_eq!(project.libraries[1].kind, LibraryKind::Shared);
    }

    #[test]
    fn test_find_package() {
        let project = parse_and_extract(
            "
            project(myapp)
            find_package(Boost 1.70 REQUIRED COMPONENTS system filesystem)
            find_package(OpenSSL REQUIRED)
        ",
        );

        assert_eq!(project.packages.len(), 2);

        let boost = &project.packages[0];
        assert_eq!(boost.name, "Boost");
        assert_eq!(boost.version, Some("1.70".to_string()));
        assert!(boost.required);
        assert_eq!(boost.components, vec!["system", "filesystem"]);

        let ssl = &project.packages[1];
        assert_eq!(ssl.name, "OpenSSL");
        assert!(ssl.required);
    }

    #[test]
    fn test_pkg_check_modules() {
        let project = parse_and_extract(
            "
            project(microdnf)
            pkg_check_modules(GLIB REQUIRED glib-2.0>=2.44.0)
            pkg_check_modules(LIBDNF REQUIRED libdnf>=0.62.0)
            add_executable(microdnf main.c)
            target_link_libraries(microdnf ${GLIB_LIBRARIES} ${LIBDNF_LIBRARIES})
        ",
        );

        assert_eq!(project.pkg_config_modules.len(), 2);

        let glib = &project.pkg_config_modules[0];
        assert_eq!(glib.prefix, "GLIB");
        assert!(glib.required);
        assert_eq!(glib.packages, vec!["glib-2.0>=2.44.0"]);

        let libdnf = &project.pkg_config_modules[1];
        assert_eq!(libdnf.prefix, "LIBDNF");
        assert!(libdnf.required);
        assert_eq!(libdnf.packages, vec!["libdnf>=0.62.0"]);

        // Check that variables were expanded in link_libraries
        let exe = &project.executables[0];
        assert_eq!(exe.link_libraries, vec!["-lglib-2.0", "-llibdnf"]);
    }

    #[test]
    fn test_compile_flags_extraction() {
        let project = parse_and_extract(
            "
            project(myapp)
            add_library(mylib STATIC lib.cpp)
            add_executable(myapp main.cpp)
            target_compile_definitions(mylib PRIVATE FOO BAR=1 -DBAZ /DWIN32)
            target_compile_options(mylib PUBLIC -O2 -Wall -Iinclude -isystem /sys /Iwin)
            target_compile_definitions(myapp INTERFACE APPDEF)
            target_compile_options(myapp PRIVATE -g)
        ",
        );

        let lib = &project.libraries[0];
        assert_eq!(
            lib.compile_definitions,
            vec!["FOO", "BAR=1", "BAZ", "WIN32"]
        );
        assert_eq!(lib.compile_options, vec!["-O2", "-Wall"]);

        let exe = &project.executables[0];
        assert_eq!(exe.compile_definitions, vec!["APPDEF"]);
        assert_eq!(exe.compile_options, vec!["-g"]);
    }

    #[test]
    fn test_skip_alias_library() {
        let project = parse_and_extract(
            "
            project(myapp)
            add_library(alias_lib ALIAS real_lib)
            add_library(real_lib STATIC lib.cpp)
        ",
        );

        assert_eq!(project.libraries.len(), 1);
        assert_eq!(project.libraries[0].name, "real_lib");
    }

    #[test]
    fn test_skip_dynamic_target_name() {
        let project = parse_and_extract(
            "
            project(myapp)
            add_executable(${PROJECT_NAME} main.cpp)
        ",
        );

        assert!(project.executables.is_empty());
    }

    #[test]
    fn test_variable_expansion_in_sources() {
        let project = parse_and_extract(
            "
            project(microdnf)
            set(DNF_SRCS dnf-command.c dnf-utils.c)
            add_executable(microdnf ${DNF_SRCS})
        ",
        );

        assert_eq!(project.executables.len(), 1);
        let exe = &project.executables[0];
        assert_eq!(exe.name, "microdnf");
        assert_eq!(exe.sources, vec!["dnf-command.c", "dnf-utils.c"]);
    }

    #[test]
    fn test_variable_expansion_with_list_append() {
        let project = parse_and_extract(
            "
            project(mylib)
            set(LIB_SRCS core.cpp)
            list(APPEND LIB_SRCS utils.cpp helper.cpp)
            add_library(mylib STATIC ${LIB_SRCS})
        ",
        );

        assert_eq!(project.libraries.len(), 1);
        let lib = &project.libraries[0];
        assert_eq!(lib.name, "mylib");
        assert_eq!(lib.sources, vec!["core.cpp", "utils.cpp", "helper.cpp"]);
    }

    #[test]
    fn test_variable_expansion_mixed_sources() {
        let project = parse_and_extract(
            "
            project(myapp)
            set(COMMON_SRCS a.c b.c)
            add_executable(myapp main.c ${COMMON_SRCS} extra.c)
        ",
        );

        assert_eq!(project.executables.len(), 1);
        let exe = &project.executables[0];
        assert_eq!(exe.name, "myapp");
        assert_eq!(exe.sources, vec!["main.c", "a.c", "b.c", "extra.c"]);
    }

    #[test]
    fn test_extract_project_from_path() {
        use std::io::Write;

        // Create a temporary directory structure
        let temp_dir = std::env::temp_dir().join("rebaze_test_recursive");
        let _ = std::fs::remove_dir_all(&temp_dir); // Clean up from previous runs
        std::fs::create_dir_all(&temp_dir).unwrap();

        // Create subdirectory
        let sub_dir = temp_dir.join("subdir");
        std::fs::create_dir_all(&sub_dir).unwrap();

        // Write root CMakeLists.txt
        let mut root_cmake = std::fs::File::create(temp_dir.join("CMakeLists.txt")).unwrap();
        writeln!(
            root_cmake,
            r#"
cmake_minimum_required(VERSION 3.10)
project(testproject)
add_library(rootlib STATIC root.cpp)
add_subdirectory(subdir)
"#
        )
        .unwrap();

        // Write subdirectory CMakeLists.txt
        let mut sub_cmake = std::fs::File::create(sub_dir.join("CMakeLists.txt")).unwrap();
        writeln!(
            sub_cmake,
            r#"
add_executable(subapp main.cpp)
add_library(sublib SHARED sub.cpp)
"#
        )
        .unwrap();

        // Test recursive extraction
        let project = extract_project_from_path(&temp_dir).unwrap();

        // Verify root project info
        assert_eq!(project.name, "testproject");
        assert_eq!(project.cmake_minimum_version, Some("3.10".to_string()));

        // Verify all targets are merged
        assert_eq!(project.libraries.len(), 2);
        assert_eq!(project.executables.len(), 1);

        // Check library names
        let lib_names: Vec<&str> = project.libraries.iter().map(|l| l.name.as_str()).collect();
        assert!(lib_names.contains(&"rootlib"));
        assert!(lib_names.contains(&"sublib"));

        // Check executable name
        assert_eq!(project.executables[0].name, "subapp");

        // Verify subdirectories are recorded
        assert_eq!(project.subdirectories, vec!["subdir"]);

        // Clean up
        std::fs::remove_dir_all(&temp_dir).unwrap();
    }

    #[test]
    fn test_extract_project_from_path_missing_subdir() {
        use std::io::Write;

        // Create a temporary directory
        let temp_dir = std::env::temp_dir().join("rebaze_test_missing_subdir");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();

        // Write CMakeLists.txt that references a non-existent subdirectory
        let mut root_cmake = std::fs::File::create(temp_dir.join("CMakeLists.txt")).unwrap();
        writeln!(
            root_cmake,
            r#"
cmake_minimum_required(VERSION 3.10)
project(testproject)
add_subdirectory(nonexistent)
"#
        )
        .unwrap();

        // Should succeed despite missing subdirectory
        let project = extract_project_from_path(&temp_dir).unwrap();
        assert_eq!(project.name, "testproject");
        assert_eq!(project.subdirectories, vec!["nonexistent"]);

        // Clean up
        std::fs::remove_dir_all(&temp_dir).unwrap();
    }

    #[test]
    fn test_extract_project_from_path_nested() {
        use std::io::Write;

        // Create a deeply nested directory structure
        let temp_dir = std::env::temp_dir().join("rebaze_test_nested");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();

        let level1 = temp_dir.join("level1");
        let level2 = level1.join("level2");
        std::fs::create_dir_all(&level2).unwrap();

        // Root CMakeLists.txt
        let mut root_cmake = std::fs::File::create(temp_dir.join("CMakeLists.txt")).unwrap();
        writeln!(
            root_cmake,
            r#"
project(root)
add_executable(root_exe main.cpp)
add_subdirectory(level1)
"#
        )
        .unwrap();

        // Level 1 CMakeLists.txt
        let mut level1_cmake = std::fs::File::create(level1.join("CMakeLists.txt")).unwrap();
        writeln!(
            level1_cmake,
            r#"
add_executable(level1_exe main.cpp)
add_subdirectory(level2)
"#
        )
        .unwrap();

        // Level 2 CMakeLists.txt
        let mut level2_cmake = std::fs::File::create(level2.join("CMakeLists.txt")).unwrap();
        writeln!(
            level2_cmake,
            r#"
add_executable(level2_exe main.cpp)
"#
        )
        .unwrap();

        // Test recursive extraction
        let project = extract_project_from_path(&temp_dir).unwrap();

        // All 3 executables should be found
        assert_eq!(project.executables.len(), 3);
        let exe_names: Vec<&str> = project.executables.iter().map(|e| e.name.as_str()).collect();
        assert!(exe_names.contains(&"root_exe"));
        assert!(exe_names.contains(&"level1_exe"));
        assert!(exe_names.contains(&"level2_exe"));

        // Clean up
        std::fs::remove_dir_all(&temp_dir).unwrap();
    }

    #[test]
    fn test_global_include_directories() {
        let project = parse_and_extract(
            "
            project(myapp)
            include_directories(include)
            include_directories(${CMAKE_CURRENT_SOURCE_DIR})
            include_directories(${CMAKE_CURRENT_SOURCE_DIR}/src)
            add_executable(myapp main.cpp)
            add_library(mylib STATIC lib.cpp)
        ",
        );

        // Check global include directories were extracted
        assert_eq!(project.global_include_directories.len(), 3);
        assert!(project.global_include_directories.contains(&"include".to_string()));
        assert!(project.global_include_directories.contains(&".".to_string()));
        assert!(project.global_include_directories.contains(&"src".to_string()));

        // Check they were applied to the executable
        let exe = &project.executables[0];
        assert!(exe.include_directories.contains(&"include".to_string()));
        assert!(exe.include_directories.contains(&".".to_string()));
        assert!(exe.include_directories.contains(&"src".to_string()));

        // Check they were applied to the library
        let lib = &project.libraries[0];
        assert!(lib.include_directories.contains(&"include".to_string()));
        assert!(lib.include_directories.contains(&".".to_string()));
        assert!(lib.include_directories.contains(&"src".to_string()));
    }

    #[test]
    fn test_global_include_directories_with_target_specific() {
        let project = parse_and_extract(
            "
            project(myapp)
            include_directories(global_include)
            add_executable(myapp main.cpp)
            target_include_directories(myapp PRIVATE target_include)
        ",
        );

        // Executable should have both global and target-specific includes
        let exe = &project.executables[0];
        assert!(exe.include_directories.contains(&"target_include".to_string()));
        assert!(exe.include_directories.contains(&"global_include".to_string()));
    }

    #[test]
    fn test_global_include_directories_skip_unexpanded() {
        let project = parse_and_extract(
            "
            project(myapp)
            include_directories(${SOME_UNKNOWN_VAR})
            include_directories(valid_dir)
            add_executable(myapp main.cpp)
        ",
        );

        // Should only have valid_dir, not the unexpanded variable
        assert_eq!(project.global_include_directories.len(), 1);
        assert!(project.global_include_directories.contains(&"valid_dir".to_string()));
    }

    #[test]
    fn test_target_sources() {
        let project = parse_and_extract(
            "
            project(mylib)
            add_library(mylib STATIC initial.cpp)
            target_sources(mylib PRIVATE extra1.cpp extra2.cpp)
            target_sources(mylib PUBLIC public.cpp)
        ",
        );

        assert_eq!(project.libraries.len(), 1);
        let lib = &project.libraries[0];
        assert_eq!(lib.name, "mylib");
        assert_eq!(
            lib.sources,
            vec!["initial.cpp", "extra1.cpp", "extra2.cpp", "public.cpp"]
        );
    }

    #[test]
    fn test_target_sources_with_file_set() {
        let project = parse_and_extract(
            "
            project(mylib)
            add_library(mylib STATIC initial.cpp)
            target_sources(mylib
                PUBLIC
                    FILE_SET HEADERS
                    TYPE HEADERS
                    BASE_DIRS include
                    FILES include/mylib.h
                PRIVATE
                    impl.cpp
            )
        ",
        );

        assert_eq!(project.libraries.len(), 1);
        let lib = &project.libraries[0];
        // FILE_SET block should be skipped, only impl.cpp should be captured
        assert_eq!(lib.sources, vec!["initial.cpp", "impl.cpp"]);
    }
}
