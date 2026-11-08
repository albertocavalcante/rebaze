//! Accepted static source semantics, separate from syntax parsing.

use std::path::Component;

use super::types::{CMakeProject, EvaluationReason, SourceIssue};
use crate::ast::{Argument, ArgumentPart, Command};
use crate::eval::EvalContext;

#[derive(Default)]
pub(super) struct Profile {
    depth: usize,
}

impl Profile {
    /// Record unsupported intent and avoid treating scoped bodies as unconditional.
    pub(super) fn accepts(
        &mut self,
        cmd: &Command,
        project: &mut CMakeProject,
        ctx: &EvalContext,
    ) -> bool {
        if matches!(
            cmd.name.as_str(),
            "if" | "foreach" | "while" | "function" | "macro" | "block"
        ) {
            self.depth += 1;
            issue(project, cmd, EvaluationReason::ControlFlow);
            return false;
        }
        if matches!(
            cmd.name.as_str(),
            "endif" | "endforeach" | "endwhile" | "endfunction" | "endmacro" | "endblock"
        ) {
            self.depth = self.depth.saturating_sub(1);
            issue(project, cmd, EvaluationReason::ControlFlow);
            return false;
        }
        if self.depth > 0 {
            return false;
        }
        let reason = match cmd.name.as_str() {
            "set"
                if cmd
                    .arg_literal(0)
                    .is_none_or(|name| name.starts_with("ENV{") || name.starts_with("CACHE{"))
                    || cmd
                        .arguments
                        .iter()
                        .skip(1)
                        .filter_map(Argument::as_literal)
                        .any(|arg| matches!(arg, "CACHE" | "PARENT_SCOPE")) =>
            {
                Some(EvaluationReason::Assignment)
            }
            "list"
                if !matches!(cmd.arg_literal(0), Some("APPEND" | "PREPEND"))
                    || cmd.arg_literal(1).is_none() =>
            {
                Some(EvaluationReason::List)
            }
            "add_subdirectory"
                if cmd.arguments.len() != 1
                    || cmd.arg_literal(0).is_none_or(|dir| {
                        dir.is_empty()
                            || std::path::Path::new(dir)
                                .components()
                                .any(|part| !matches!(part, Component::Normal(_)))
                    }) =>
            {
                Some(EvaluationReason::Subdirectory)
            }
            "cmake_minimum_required"
            | "project"
            | "set"
            | "list"
            | "add_executable"
            | "add_library"
            | "find_package"
            | "pkg_check_modules"
            | "add_subdirectory"
            | "include_directories"
            | "target_compile_features"
            | "target_link_libraries"
            | "target_include_directories"
            | "target_compile_definitions"
            | "target_compile_options"
            | "target_sources"
            | "enable_testing"
            | "add_test"
            | "set_tests_properties" => None,
            "include"
                if cmd.arguments.len() == 1 && cmd.arg_literal(0) == Some("FindPkgConfig") =>
            {
                None
            }
            _ => Some(EvaluationReason::Command),
        };
        if let Some(reason) = reason {
            issue(project, cmd, reason);
            return false;
        }
        let assigned_variable = match cmd.name.as_str() {
            "set" => cmd.arg_literal(0),
            "list" => cmd.arg_literal(1),
            _ => None,
        };
        if assigned_variable.is_some_and(|name| {
            name == "BUILD_SHARED_LIBS"
                || name.starts_with("CMAKE_")
                    && !(cmd.name == "set"
                        && matches!(
                            name,
                            "CMAKE_C_STANDARD"
                                | "CMAKE_CXX_STANDARD"
                                | "CMAKE_TEST_LAUNCHER"
                                | "CMAKE_CROSSCOMPILING_EMULATOR"
                        ))
        }) {
            issue(project, cmd, EvaluationReason::Module);
            return false;
        }
        if matches!(
            assigned_variable,
            Some("CMAKE_C_STANDARD" | "CMAKE_CXX_STANDARD")
        ) && cmd
            .arguments
            .iter()
            .skip(1)
            .flat_map(|arg| ctx.expand_argument(arg))
            .count()
            > 1
        {
            issue(project, cmd, EvaluationReason::Assignment);
            return false;
        }
        if cmd.name == "include" && ctx.get("CMAKE_MODULE_PATH").is_some() {
            issue(project, cmd, EvaluationReason::Module);
            return false;
        }
        if matches!(
            cmd.name.as_str(),
            "cmake_minimum_required"
                | "project"
                | "find_package"
                | "pkg_check_modules"
                | "target_compile_features"
        ) && cmd.arguments.iter().any(|arg| arg.as_literal().is_none())
        {
            issue(project, cmd, EvaluationReason::Name);
            return false;
        }
        if cmd.name == "set"
            && cmd
                .arguments
                .iter()
                .skip(1)
                .flat_map(|arg| ctx.expand_argument(arg))
                .any(|value| matches!(value.as_str(), "CACHE" | "PARENT_SCOPE"))
        {
            issue(project, cmd, EvaluationReason::Assignment);
            return false;
        }
        if matches!(
            cmd.name.as_str(),
            "project" | "add_executable" | "add_library" | "find_package" | "pkg_check_modules"
        ) && cmd.arg_literal(0).is_none()
        {
            issue(project, cmd, EvaluationReason::Name);
            return false;
        }
        // Test registration has its own stricter literal argv validation.
        if cmd.name != "add_test" && cmd.arguments.iter().any(unsupported_expansion) {
            issue(project, cmd, EvaluationReason::Expansion);
            return false;
        }
        if cmd.name != "add_test" {
            for arg in &cmd.arguments {
                if arg.to_string_with_vars().contains(';')
                    || ctx
                        .expand_argument(arg)
                        .iter()
                        .any(|value| value.contains(';'))
                    || matches!(arg, Argument::Quoted(value) if value.parts.iter().any(|part| matches!(part, ArgumentPart::Variable(_))))
                {
                    issue(project, cmd, EvaluationReason::ArgumentList);
                    return false;
                }
                if let Argument::Quoted(value) | Argument::Unquoted(value) = arg {
                    for part in &value.parts {
                        if let ArgumentPart::Variable(name) = part
                            && ctx.get(name).is_none()
                            && !(name == "CMAKE_CURRENT_SOURCE_DIR"
                                && matches!(
                                    cmd.name.as_str(),
                                    "include_directories" | "target_include_directories"
                                ))
                        {
                            // Retain useful syntax observations, but never migrate a
                            // source stream that depends on unobserved variable state.
                            issue(project, cmd, EvaluationReason::Expansion);
                        }
                    }
                }
            }
        }
        if cmd.name.starts_with("target_")
            && !cmd.arg_literal(0).is_some_and(|target| {
                project.executables.iter().any(|exe| exe.name == target)
                    || project.libraries.iter().any(|lib| lib.name == target)
            })
        {
            issue(project, cmd, EvaluationReason::TargetScope);
            return false;
        }
        true
    }
}

fn unsupported_expansion(arg: &Argument) -> bool {
    match arg {
        Argument::Bracket(text) => text.contains("$<"),
        Argument::Quoted(value) | Argument::Unquoted(value) => {
            value.parts.iter().any(|part| match part {
                ArgumentPart::Text(text) => text.contains("$<"),
                ArgumentPart::Variable(name) => name.contains('$'),
                ArgumentPart::EnvVariable(_)
                | ArgumentPart::CacheVariable(_)
                | ArgumentPart::GeneratorExpr(_) => true,
            })
        }
    }
}

pub(super) fn issue(project: &mut CMakeProject, cmd: &Command, reason: EvaluationReason) {
    project.requires_evaluation.push(SourceIssue {
        path: project.path.join("CMakeLists.txt"),
        offset: cmd.span.start,
        command: cmd.name.clone(),
        reason,
    });
}
