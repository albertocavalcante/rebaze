//! CMake file parser for rebaze.
//!
//! This crate provides a parser for CMakeLists.txt files using
//! chumsky parser combinators. It extracts project structure,
//! targets, and dependencies for migration to Bazel.

use std::path::Path;

use anyhow::Result;
use ariadne::{Color, Label, Report, ReportKind, Source};

pub mod ast;
pub mod eval;
mod parser;
mod project;

pub use ast::{Argument, ArgumentPart, ArgumentValue, CMakeFile, Command};
pub use eval::EvalContext;
pub use project::{
    extract_project_from_path, CMakeProject, Executable, ExtractError, Library, LibraryKind,
    Package, PkgConfigModule,
};

/// Parse a CMake project at the given path.
///
/// Recursively parses CMakeLists.txt in the given directory and all
/// subdirectories referenced via add_subdirectory() commands.
pub fn parse(path: &Path) -> Result<CMakeProject> {
    let cmake_file = path.join("CMakeLists.txt");

    if !cmake_file.exists() {
        anyhow::bail!("No CMakeLists.txt found at {}", path.display());
    }

    tracing::debug!("Parsing CMake project at {}", path.display());

    // Use recursive parsing to follow add_subdirectory() calls
    let project = project::extract_project_from_path(path).map_err(|e| match e {
        project::ExtractError::ReadFile { path: p, source } => {
            anyhow::anyhow!("Failed to read {}: {}", p.display(), source)
        }
        project::ExtractError::Canonicalize { path: p, source } => {
            anyhow::anyhow!("Failed to canonicalize {}: {}", p.display(), source)
        }
        project::ExtractError::NotFound(p) => {
            anyhow::anyhow!("CMakeLists.txt not found at {}", p.display())
        }
        project::ExtractError::Parse { path: p, message } => {
            anyhow::anyhow!("Failed to parse {}: {}", p.display(), message)
        }
    })?;

    tracing::info!(
        "Parsed CMake project '{}' with {} executables and {} libraries",
        project.name,
        project.executables.len(),
        project.libraries.len()
    );

    Ok(project)
}

/// Parse CMake source directly (for testing or embedded CMake).
pub fn parse_source(src: &str) -> Result<CMakeFile> {
    let (file, errors) = parser::parse(src);

    if !errors.is_empty() {
        // Format errors for display
        let error_msgs: Vec<String> = errors
            .iter()
            .map(|e| format!("at {}: {e}", e.span().start))
            .collect();
        anyhow::bail!("Parse errors:\n{}", error_msgs.join("\n"));
    }

    file.ok_or_else(|| anyhow::anyhow!("Parse failed"))
}

/// Report parsing errors with nice formatting.
/// Currently unused but kept for debugging and future interactive error reporting.
#[allow(dead_code)]
fn report_errors(filename: &str, src: &str, errors: &[chumsky::error::Simple<char>]) {
    for err in errors {
        let report = Report::build(ReportKind::Error, filename, err.span().start)
            .with_message("Failed to parse CMake file")
            .with_label(
                Label::new((filename, err.span()))
                    .with_message(format!("{err}"))
                    .with_color(Color::Red),
            );

        let report = if let Some(Some(expected)) = err.expected().next().map(|e| e.as_ref()) {
            report.with_help(format!("Expected '{expected}'"))
        } else {
            report
        };

        // Print to stderr
        report.finish().eprint((filename, Source::from(src))).ok();
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_source() {
        let src = "
cmake_minimum_required(VERSION 3.20)
project(myapp VERSION 1.0.0 LANGUAGES CXX)

add_executable(myapp
    src/main.cpp
    src/util.cpp
)

target_link_libraries(myapp PRIVATE pthread)
";

        let file = parse_source(src).unwrap();
        assert_eq!(file.commands.len(), 4);
    }

    #[test]
    fn test_real_world_cmake() {
        // A more realistic CMakeLists.txt
        let src = r#"
cmake_minimum_required(VERSION 3.16)

project(awesome_app
    VERSION 2.1.0
    DESCRIPTION "An awesome application"
    LANGUAGES CXX
)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)

find_package(Boost 1.70 REQUIRED COMPONENTS system filesystem)
find_package(OpenSSL REQUIRED)
find_package(Threads REQUIRED)

add_library(core STATIC
    src/core/engine.cpp
    src/core/config.cpp
)

target_include_directories(core PUBLIC
    ${CMAKE_CURRENT_SOURCE_DIR}/include
)

add_executable(app src/main.cpp)

target_link_libraries(app PRIVATE
    core
    Boost::system
    Boost::filesystem
    OpenSSL::SSL
    Threads::Threads
)

add_subdirectory(tests)
"#;

        let file = parse_source(src).unwrap();

        // Verify we parsed all commands
        let cmd_names: Vec<&str> = file.commands.iter().map(|c| c.name.as_str()).collect();
        assert!(cmd_names.contains(&"cmake_minimum_required"));
        assert!(cmd_names.contains(&"project"));
        assert!(cmd_names.contains(&"set"));
        assert!(cmd_names.contains(&"find_package"));
        assert!(cmd_names.contains(&"add_library"));
        assert!(cmd_names.contains(&"add_executable"));
        assert!(cmd_names.contains(&"target_link_libraries"));
        assert!(cmd_names.contains(&"add_subdirectory"));
    }
}
