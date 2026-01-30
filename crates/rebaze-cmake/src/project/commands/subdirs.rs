//! Subdirectory and include directory extraction.
//!
//! Handles extraction of add_subdirectory() and include_directories() commands.

use super::super::path_utils::normalize_include_path;
use super::super::types::CMakeProject;
use crate::ast::Command;
use crate::eval::EvalContext;

/// Extract add_subdirectory() command.
pub fn extract_subdirectory(cmd: &Command, project: &mut CMakeProject) {
    if let Some(dir) = cmd.arg_literal(0) {
        project.subdirectories.push(dir.to_string());
    }
}

/// Extract include_directories() command for global includes.
pub fn extract_global_includes(cmd: &Command, project: &mut CMakeProject, ctx: &EvalContext) {
    // include_directories([AFTER|BEFORE] [SYSTEM] dir1 [dir2 ...])
    for arg in &cmd.arguments {
        // Try to get as literal first
        if let Some(lit) = arg.as_literal() {
            // Skip keywords
            if matches!(lit.to_uppercase().as_str(), "AFTER" | "BEFORE" | "SYSTEM") {
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
            if !matches!(raw.to_uppercase().as_str(), "AFTER" | "BEFORE" | "SYSTEM") {
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
                if matches!(val.to_uppercase().as_str(), "AFTER" | "BEFORE" | "SYSTEM") {
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
