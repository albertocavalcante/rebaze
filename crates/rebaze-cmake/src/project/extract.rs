//! Project extraction logic.
//!
//! Core functions for extracting CMake project information from parsed AST.

use std::collections::HashSet;
use std::path::Path;
use std::path::PathBuf;

use super::commands;
use super::path_utils::prefix_path;
use super::source_profile::{Profile, issue};
use super::target_props;
use super::types::{CMakeProject, EvaluationReason, ExtractError};
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
    let (mut project, subdirectories) = extract_with_subdirectories(file, path, ctx);
    if !subdirectories.is_empty() {
        project.requires_evaluation.push(super::types::SourceIssue {
            path: project.path.join("CMakeLists.txt"),
            offset: 0,
            command: "add_subdirectory".into(),
            reason: EvaluationReason::UnvisitedSubdirectory,
        });
    }
    project
}

struct Subdirectory {
    name: String,
    context: EvalContext,
}

fn extract_with_subdirectories(
    file: &CMakeFile,
    path: PathBuf,
    ctx: &mut EvalContext,
) -> (CMakeProject, Vec<Subdirectory>) {
    let mut project = CMakeProject {
        testing: super::testing::extract(file),
        path,
        ..Default::default()
    };
    let mut profile = Profile::default();
    let mut subdirectories = Vec::new();
    let mut target_standards = None;
    for cmd in &file.commands {
        if !profile.accepts(cmd, &mut project, ctx) {
            continue;
        }
        match cmd.name.as_str() {
            "set" | "list" => crate::eval::evaluate_command(cmd, ctx),
            "cmake_minimum_required" => commands::extract_cmake_version(cmd, &mut project),
            "project" => commands::extract_project_info(cmd, &mut project),
            "add_executable" | "add_library" => {
                let target_count = project.executables.len() + project.libraries.len();
                let standards = (
                    ctx.get("CMAKE_C_STANDARD").cloned(),
                    ctx.get("CMAKE_CXX_STANDARD").cloned(),
                );
                if cmd.name == "add_executable" {
                    commands::extract_executable(cmd, &mut project, ctx);
                } else {
                    commands::extract_library(cmd, &mut project, ctx);
                }
                if project.executables.len() + project.libraries.len() > target_count {
                    if target_standards
                        .as_ref()
                        .is_some_and(|existing| existing != &standards)
                    {
                        issue(&mut project, cmd, EvaluationReason::LanguageStandard);
                    }
                    target_standards = Some(standards);
                }
            }
            "find_package" => {
                // Keep metadata useful to analysis, but a module/config script
                // can change arbitrary variables before subsequent statements.
                issue(&mut project, cmd, EvaluationReason::Module);
                commands::extract_package(cmd, &mut project);
            }
            "pkg_check_modules" => commands::extract_pkg_config(cmd, &mut project, ctx),
            "add_subdirectory" => {
                commands::extract_subdirectory(cmd, &mut project);
                if !project.global_include_directories.is_empty() {
                    issue(&mut project, cmd, EvaluationReason::DirectoryIncludes);
                }
                subdirectories.push(Subdirectory {
                    name: cmd.arg_literal(0).unwrap_or_default().into(),
                    context: ctx.clone(),
                });
            }
            "include_directories" => commands::extract_global_includes(cmd, &mut project, ctx),
            "target_compile_features" => commands::extract_compile_features(cmd, &mut project),
            "target_link_libraries" => target_props::apply_link_libraries(cmd, &mut project, ctx),
            "target_include_directories" => {
                target_props::apply_include_directories(cmd, &mut project, ctx);
            }
            "target_compile_definitions" => {
                target_props::apply_compile_definitions(cmd, &mut project, ctx);
            }
            "target_compile_options" => target_props::apply_compile_options(cmd, &mut project, ctx),
            "target_sources" => target_props::apply_target_sources(cmd, &mut project, ctx),
            _ => {}
        }
    }
    // CMake initializes standards when a target is created, not from a later set().
    if let Some((c, cxx)) = target_standards {
        project.c_standard = project
            .c_standard
            .or_else(|| c.and_then(|values| values.first().cloned()));
        project.cxx_standard = project
            .cxx_standard
            .or_else(|| cxx.and_then(|values| values.first().cloned()));
    }

    // CMake updates existing directory targets and initializes later local ones.
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

    (project, subdirectories)
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
    extract_recursive(root, &mut visited, &mut EvalContext::new())
}

/// Recursively extract project information from a directory.
fn extract_recursive(
    dir: &Path,
    visited: &mut HashSet<PathBuf>,
    ctx: &mut EvalContext,
) -> Result<CMakeProject, ExtractError> {
    // Canonicalize to avoid visiting the same directory twice via symlinks
    let canonical = dir.canonicalize().map_err(|e| ExtractError::Canonicalize {
        path: dir.to_path_buf(),
        source: e,
    })?;

    // Skip if already visited (prevents infinite loops)
    if !visited.insert(canonical) {
        return Err(ExtractError::RepeatedDirectory(dir.to_path_buf()));
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
        let error_msgs: Vec<String> = errors.iter().map(crate::parser::describe_error).collect();
        return Err(ExtractError::Parse {
            path: cmake_path,
            message: error_msgs.join("; "),
        });
    }

    let file = file.ok_or_else(|| ExtractError::Parse {
        path: cmake_path.clone(),
        message: "Parse returned no file".to_string(),
    })?;

    let (mut project, subdirectories) = extract_with_subdirectories(&file, dir.to_path_buf(), ctx);
    let mut has_target_standards = !project.executables.is_empty() || !project.libraries.is_empty();

    // Each child receives the variable state observed at its call, with isolated
    // local mutations. Parent-scope/cache effects are rejected by the profile.
    for mut subdirectory in subdirectories {
        let subdir_name = &subdirectory.name;
        let subdir_path = dir.join(subdir_name);
        let subproject = extract_recursive(&subdir_path, visited, &mut subdirectory.context)?;
        project
            .requires_evaluation
            .extend(subproject.requires_evaluation);
        if !subproject.executables.is_empty() || !subproject.libraries.is_empty() {
            if !has_target_standards {
                project.c_standard.clone_from(&subproject.c_standard);
                project.cxx_standard.clone_from(&subproject.cxx_standard);
                has_target_standards = true;
            } else if subproject.c_standard != project.c_standard
                || subproject.cxx_standard != project.cxx_standard
            {
                project.requires_evaluation.push(super::types::SourceIssue {
                    path: subdir_path.join("CMakeLists.txt"),
                    offset: 0,
                    command: "add_subdirectory".into(),
                    reason: EvaluationReason::LanguageStandard,
                });
            }
        }
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
        project.aliases.extend(subproject.aliases);
        project.packages.extend(subproject.packages);
        project.testing.tests.extend(subproject.testing.tests);
        project
            .testing
            .requires_evaluation
            .extend(subproject.testing.requires_evaluation);
        project
            .testing
            .unsupported
            .extend(subproject.testing.unsupported);
        project
            .pkg_config_modules
            .extend(subproject.pkg_config_modules);
        // Note: We don't merge project name/version from subdirectories
    }

    Ok(project)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Fixture(PathBuf);

    impl Fixture {
        fn new(source: &str) -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let path = std::env::temp_dir().join(format!(
                "rebaze-scope-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir(&path).unwrap();
            let fixture = Self(path);
            fixture.write("CMakeLists.txt", source);
            fixture
        }

        fn write(&self, name: &str, source: &str) {
            let path = self.0.join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, source).unwrap();
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn variables_are_observed_at_each_command() {
        let file = crate::parse_source(
            "set(SRC early.c)\nadd_library(one STATIC ${SRC})\n\
             set(SRC late.c)\nadd_library(two STATIC ${SRC})\n\
             set(DEFS EARLY)\ntarget_compile_definitions(one PRIVATE ${DEFS})\n\
             set(DEFS LATE)\ntarget_compile_definitions(two PRIVATE ${DEFS})\n\
             list(APPEND SRC last.c)\ntarget_sources(two PRIVATE ${SRC})",
        )
        .unwrap();
        let project = extract_project(&file, PathBuf::from("."));
        assert_eq!(project.libraries[0].sources, ["early.c"]);
        assert_eq!(project.libraries[1].sources, ["late.c", "late.c", "last.c"]);
        assert_eq!(project.libraries[0].compile_definitions, ["EARLY"]);
        assert_eq!(project.libraries[1].compile_definitions, ["LATE"]);
    }

    #[test]
    fn child_scope_inherits_at_call_and_does_not_modify_parent() {
        let fixture = Fixture::new(
            "set(SRC early.c)\nadd_subdirectory(child)\n\
             add_library(parent STATIC ${SRC})\nset(SRC late.c)",
        );
        fixture.write(
            "child/CMakeLists.txt",
            "add_library(child STATIC ${SRC})\nset(SRC child_only.c)",
        );
        let project = extract_project_from_path(&fixture.0).unwrap();
        let child = project
            .libraries
            .iter()
            .find(|lib| lib.name == "child")
            .unwrap();
        let parent = project
            .libraries
            .iter()
            .find(|lib| lib.name == "parent")
            .unwrap();
        assert_eq!(child.sources, ["child/early.c"]);
        assert_eq!(parent.sources, ["early.c"]);
    }

    #[test]
    fn child_only_targets_preserve_inherited_language_standard() {
        let fixture = Fixture::new("set(CMAKE_C_STANDARD 17)\nadd_subdirectory(child)");
        fixture.write("child/CMakeLists.txt", "add_library(child STATIC child.c)");
        let project = extract_project_from_path(&fixture.0).unwrap();
        assert_eq!(project.c_standard.as_deref(), Some("17"));
        assert!(project.requires_evaluation.is_empty());
    }

    #[test]
    fn repeated_directory_is_not_silently_omitted() {
        let fixture = Fixture::new("add_subdirectory(child)\nadd_subdirectory(child)");
        fixture.write("child/CMakeLists.txt", "add_library(child STATIC child.c)");
        assert!(matches!(
            extract_project_from_path(&fixture.0),
            Err(ExtractError::RepeatedDirectory(_))
        ));
    }

    #[test]
    fn required_subdirectory_without_listfile_is_an_error() {
        let fixture = Fixture::new("add_subdirectory(child)");
        std::fs::create_dir(fixture.0.join("child")).unwrap();
        assert!(extract_project_from_path(&fixture.0).is_err());
    }
}
