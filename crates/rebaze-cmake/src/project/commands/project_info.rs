//! CMake project and version info extraction.
//!
//! Handles extraction of cmake_minimum_required() and project() commands.

use super::super::types::CMakeProject;
use crate::ast::{Argument, Command};

/// Extract cmake_minimum_required VERSION.
pub fn extract_cmake_version(cmd: &Command, project: &mut CMakeProject) {
    // cmake_minimum_required(VERSION x.y.z)
    for i in 0..cmd.arguments.len() {
        let Some(arg) = cmd.arguments[i].as_literal() else {
            continue;
        };
        if arg.eq_ignore_ascii_case("VERSION")
            && let Some(version) = cmd.arguments.get(i + 1).and_then(Argument::as_literal)
        {
            project.cmake_minimum_version = Some(version.to_string());
            return;
        }
    }
}

/// Extract project() command information.
pub fn extract_project_info(cmd: &Command, project: &mut CMakeProject) {
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
                if let Some(ver) = cmd.arguments.get(i + 1).and_then(Argument::as_literal) {
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

/// Check if the argument is a project() keyword.
fn is_project_keyword(arg: &str) -> bool {
    matches!(
        arg.to_uppercase().as_str(),
        "VERSION" | "DESCRIPTION" | "HOMEPAGE_URL" | "LANGUAGES"
    )
}

/// Check if the argument looks like a CMake language.
fn is_language(arg: &str) -> bool {
    matches!(
        arg.to_uppercase().as_str(),
        "C" | "CXX" | "CUDA" | "OBJC" | "OBJCXX" | "Fortran" | "HIP" | "ISPC" | "ASM" | "NONE"
    )
}
