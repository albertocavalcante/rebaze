//! Target property application.
//!
//! Functions to apply target_* commands to existing targets.

use super::path_utils::{
    extend_unique, is_include_flag, normalize_include_path, strip_define_prefix,
};
use super::types::CMakeProject;
use crate::ast::{Argument, Command};
use crate::eval::EvalContext;

/// Apply target_link_libraries() to a target.
pub fn apply_link_libraries(cmd: &Command, project: &mut CMakeProject, ctx: &EvalContext) {
    // target_link_libraries(target [PUBLIC|PRIVATE|INTERFACE] lib1 lib2...)
    let target = match cmd.arguments.first().and_then(Argument::as_literal) {
        Some(target) => target.to_string(),
        None => return,
    };

    let libs = collect_arguments(
        &cmd.arguments[1..],
        ctx,
        &["PUBLIC", "PRIVATE", "INTERFACE"],
    );

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

/// Apply target_include_directories() to a target.
pub fn apply_include_directories(cmd: &Command, project: &mut CMakeProject, ctx: &EvalContext) {
    // target_include_directories(target [PUBLIC|PRIVATE|INTERFACE] dir1 dir2...)
    let target = match cmd.arguments.first().and_then(Argument::as_literal) {
        Some(target) => target.to_string(),
        None => return,
    };

    let skip_keywords = [
        "PUBLIC",
        "PRIVATE",
        "INTERFACE",
        "SYSTEM",
        "BEFORE",
        "AFTER",
    ];
    let mut dirs = Vec::new();

    for arg in cmd.arguments.iter().skip(1) {
        if let Some(lit) = arg.as_literal() {
            if skip_keywords.iter().any(|k| k.eq_ignore_ascii_case(lit)) {
                continue;
            }
            let normalized = normalize_include_path(lit);
            if !normalized.is_empty() {
                dirs.push(normalized);
            }
        } else {
            let expanded = ctx.expand_argument(arg);
            for val in expanded {
                if skip_keywords.iter().any(|k| k.eq_ignore_ascii_case(&val)) {
                    continue;
                }
                let normalized = normalize_include_path(&val);
                if !normalized.is_empty() {
                    dirs.push(normalized);
                }
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

/// Apply target_compile_definitions() to a target.
pub fn apply_compile_definitions(cmd: &Command, project: &mut CMakeProject, ctx: &EvalContext) {
    // target_compile_definitions(target [PUBLIC|PRIVATE|INTERFACE] def1 def2...)
    let target = match cmd.arguments.first().and_then(Argument::as_literal) {
        Some(target) => target.to_string(),
        None => return,
    };

    let skip_keywords = ["PUBLIC", "PRIVATE", "INTERFACE"];
    let mut defs = Vec::new();

    for arg in cmd.arguments.iter().skip(1) {
        if let Some(lit) = arg.as_literal() {
            if skip_keywords.iter().any(|k| k.eq_ignore_ascii_case(lit)) {
                continue;
            }
            let def = strip_define_prefix(lit);
            if !def.is_empty() {
                defs.push(def);
            }
        } else {
            let expanded = ctx.expand_argument(arg);
            for val in expanded {
                if skip_keywords.iter().any(|k| k.eq_ignore_ascii_case(&val)) {
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

/// Apply target_compile_options() to a target.
pub fn apply_compile_options(cmd: &Command, project: &mut CMakeProject, ctx: &EvalContext) {
    // target_compile_options(target [BEFORE] [SYSTEM] [PUBLIC|PRIVATE|INTERFACE] opt1 opt2...)
    let target = match cmd.arguments.first().and_then(Argument::as_literal) {
        Some(target) => target.to_string(),
        None => return,
    };

    let skip_keywords = ["PUBLIC", "PRIVATE", "INTERFACE", "SYSTEM", "BEFORE"];
    let mut opts = Vec::new();
    let mut skip_next = false;

    for arg in cmd.arguments.iter().skip(1) {
        if skip_next {
            skip_next = false;
            continue;
        }
        if let Some(lit) = arg.as_literal() {
            if skip_keywords.iter().any(|k| k.eq_ignore_ascii_case(lit)) {
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
            let expanded = ctx.expand_argument(arg);
            for val in expanded {
                if skip_keywords.iter().any(|k| k.eq_ignore_ascii_case(&val)) {
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

/// Apply target_sources() to a target.
pub fn apply_target_sources(cmd: &Command, project: &mut CMakeProject, ctx: &EvalContext) {
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
            if matches!(upper.as_str(), "FILE_SET" | "TYPE" | "BASE_DIRS" | "FILES") {
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
            let expanded = ctx.expand_argument(arg);
            for val in expanded {
                let upper = val.to_uppercase();
                if matches!(upper.as_str(), "PUBLIC" | "PRIVATE" | "INTERFACE") {
                    skip_until_next_visibility = false;
                    continue;
                }
                if matches!(upper.as_str(), "FILE_SET" | "TYPE" | "BASE_DIRS" | "FILES") {
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

/// Collect arguments, skipping specified keywords.
fn collect_arguments(args: &[Argument], ctx: &EvalContext, skip_keywords: &[&str]) -> Vec<String> {
    let mut result = Vec::new();

    for arg in args {
        if let Some(lit) = arg.as_literal() {
            if skip_keywords.iter().any(|k| k.eq_ignore_ascii_case(lit)) {
                continue;
            }
            result.push(lit.to_string());
        } else {
            let expanded = ctx.expand_argument(arg);
            for val in expanded {
                if skip_keywords.iter().any(|k| k.eq_ignore_ascii_case(&val)) {
                    continue;
                }
                result.push(val);
            }
        }
    }

    result
}
