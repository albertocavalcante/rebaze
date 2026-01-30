//! Project extraction logic.
//!
//! Core functions for extracting CMake project information from parsed AST.

use std::collections::HashSet;
use std::path::Path;
use std::path::PathBuf;

use super::commands;
use super::path_utils::prefix_path;
use super::target_props;
use super::types::{CMakeProject, ExtractError};
use crate::ast::CMakeFile;
use crate::eval::EvalContext;

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
    if let Some(values) = ctx.get("CMAKE_C_STANDARD")
        && let Some(std) = values.first()
    {
        project.c_standard = Some(std.clone());
    }

    // First pass: extract basic project info and targets
    for cmd in &file.commands {
        match cmd.name.as_str() {
            "cmake_minimum_required" => commands::extract_cmake_version(cmd, &mut project),
            "project" => commands::extract_project_info(cmd, &mut project),
            "add_executable" => commands::extract_executable(cmd, &mut project, ctx),
            "add_library" => commands::extract_library(cmd, &mut project, ctx),
            "find_package" => commands::extract_package(cmd, &mut project),
            "pkg_check_modules" => commands::extract_pkg_config(cmd, &mut project, ctx),
            "add_subdirectory" => commands::extract_subdirectory(cmd, &mut project),
            "include_directories" => commands::extract_global_includes(cmd, &mut project, ctx),
            "target_compile_features" => commands::extract_compile_features(cmd, &mut project),
            _ => {}
        }
    }

    // Second pass: attach target properties
    for cmd in &file.commands {
        match cmd.name.as_str() {
            "target_link_libraries" => target_props::apply_link_libraries(cmd, &mut project, ctx),
            "target_include_directories" => {
                target_props::apply_include_directories(cmd, &mut project, ctx);
            }
            "target_compile_definitions" => {
                target_props::apply_compile_definitions(cmd, &mut project, ctx);
            }
            "target_compile_options" => {
                target_props::apply_compile_options(cmd, &mut project, ctx);
            }
            "target_sources" => target_props::apply_target_sources(cmd, &mut project, ctx),
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
                project
                    .pkg_config_modules
                    .extend(subproject.pkg_config_modules);
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
