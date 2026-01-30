//! Executable and library target extraction.
//!
//! Handles extraction of add_executable(), add_library(), and target_compile_features().

use super::super::types::{CMakeProject, Executable, Library, LibraryKind};
use crate::ast::{Argument, Command};
use crate::eval::EvalContext;

/// Extract add_executable() command.
pub fn extract_executable(cmd: &Command, project: &mut CMakeProject, ctx: &EvalContext) {
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

    let sources = extract_sources(
        &cmd.arguments[1..],
        ctx,
        &["WIN32", "MACOSX_BUNDLE", "EXCLUDE_FROM_ALL"],
    );

    project
        .executables
        .push(Executable::with_sources(name, sources));
}

/// Extract add_library() command.
pub fn extract_library(cmd: &Command, project: &mut CMakeProject, ctx: &EvalContext) {
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
    let mut is_imported = false;
    let mut is_alias = false;

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
                "IMPORTED" => is_imported = true,
                "ALIAS" => is_alias = true,
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
                    "IMPORTED" => is_imported = true,
                    "ALIAS" => is_alias = true,
                    _ => sources.push(val),
                }
            }
        }
    }

    // Handle alias: add_library(alias_name ALIAS real_target)
    // Record the mapping but don't create a library target
    if is_alias {
        if let Some(real_target) = sources.into_iter().next() {
            tracing::debug!("Recording alias '{name}' -> '{real_target}'");
            project.aliases.insert(name, real_target);
        }
        return;
    }

    if is_imported {
        tracing::debug!("Skipping imported library target '{name}'");
        return;
    }

    project
        .libraries
        .push(Library::with_sources(name, kind, sources));
}

/// Extract compile features like cxx_std_17 from target_compile_features() commands.
pub fn extract_compile_features(cmd: &Command, project: &mut CMakeProject) {
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

/// Helper to extract sources from arguments, filtering out keywords.
fn extract_sources(args: &[Argument], ctx: &EvalContext, skip_keywords: &[&str]) -> Vec<String> {
    let mut sources = Vec::new();

    for arg in args {
        if let Some(lit) = arg.as_literal() {
            if skip_keywords.iter().any(|k| k.eq_ignore_ascii_case(lit)) {
                continue;
            }
            sources.push(lit.to_string());
        } else {
            let expanded = ctx.expand_argument(arg);
            for val in expanded {
                if skip_keywords.iter().any(|k| k.eq_ignore_ascii_case(&val)) {
                    continue;
                }
                sources.push(val);
            }
        }
    }

    sources
}
