//! Conservative source-level extraction. Conditions and properties need evaluation.

use crate::ast::{Argument, CMakeFile};

use super::types::{CMakeTest, TestEvaluation, Testing};

pub(super) fn extract(file: &CMakeFile) -> Testing {
    let mut testing = Testing::default();
    let mut depth: usize = 0;
    for command in &file.commands {
        match command.name.as_str() {
            "endif" | "endforeach" | "endwhile" | "endfunction" | "endmacro" | "endblock" => {
                depth = depth.saturating_sub(1);
            }
            "if" | "foreach" | "while" | "function" | "macro" | "block" => depth += 1,
            "enable_testing" if depth == 0 => testing.enabled = true,
            "add_subdirectory" if depth > 0 => testing
                .requires_evaluation
                .push(TestEvaluation::ScopedSubdirectory),
            "add_test" if depth == 0 => match registration(&command.arguments) {
                Ok(test) => testing.tests.push(test),
                Err(reason) => testing.unsupported.push(reason.into()),
            },
            "add_test" | "enable_testing" => testing.unsupported.push(format!(
                "conditional or scoped {} requires CMake evaluation",
                command.name
            )),
            "set_tests_properties" => testing
                .unsupported
                .push("set_tests_properties is not supported by native test generation".into()),
            "set_property" if command.arg_literal(0) == Some("TEST") => {
                testing
                    .unsupported
                    .push("set_property(TEST ...) is not supported".into());
            }
            _ => {}
        }
    }
    for command in &file.commands {
        if matches!(
            command.name.as_str(),
            "set_property" | "set_target_properties"
        ) && command
            .arguments
            .iter()
            .any(|arg| arg.as_literal().is_none())
        {
            testing
                .requires_evaluation
                .push(TestEvaluation::DynamicProperty);
        }
        if command.name == "set"
            && command
                .arguments
                .first()
                .is_some_and(|arg| arg.as_literal().is_none())
        {
            testing
                .requires_evaluation
                .push(TestEvaluation::DynamicProperty);
        }
        if matches!(
            command.name.as_str(),
            "set_property" | "set_target_properties"
        ) && command
            .arguments
            .iter()
            .filter_map(Argument::as_literal)
            .any(|arg| matches!(arg, "TEST_LAUNCHER" | "CROSSCOMPILING_EMULATOR"))
        {
            testing.requires_evaluation.push(TestEvaluation::Launcher);
        }
        if command.name == "set"
            && matches!(
                command.arg_literal(0),
                Some("CMAKE_TEST_LAUNCHER" | "CMAKE_CROSSCOMPILING_EMULATOR")
            )
        {
            testing.requires_evaluation.push(TestEvaluation::Launcher);
        }
    }
    testing
}

fn registration(arguments: &[Argument]) -> Result<CMakeTest, &'static str> {
    let args: Option<Vec<_>> = arguments
        .iter()
        .map(|arg| match arg {
            Argument::Quoted(value) if value.parts.is_empty() => Some(""),
            _ => arg.as_literal(),
        })
        .collect();
    let args = args.ok_or(
        "add_test requires literal arguments; variables/generator expressions need evaluation",
    )?;
    if args.iter().any(|arg| arg.contains("$<")) {
        return Err(
            "add_test generator expressions require CMake evaluation, including bracket arguments",
        );
    }
    if args.len() < 4 || args[0] != "NAME" || args[2] != "COMMAND" {
        return Err(
            "supported test syntax: add_test(NAME <name> COMMAND <executable-target> [args...])",
        );
    }
    if args[4..].iter().any(|arg| {
        matches!(
            *arg,
            "CONFIGURATIONS" | "WORKING_DIRECTORY" | "COMMAND_EXPAND_LISTS"
        )
    }) {
        return Err(
            "add_test configurations, working directories and list expansion are not supported",
        );
    }
    Ok(CMakeTest {
        name: args[1].into(),
        executable: args[3].into(),
        args: args[4..].iter().map(|arg| (*arg).into()).collect(),
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn testing(source: &str) -> Testing {
        extract(&crate::parse_source(source).unwrap())
    }

    #[test]
    fn preserves_literal_registration_and_argument_boundaries() {
        let result =
            testing("enable_testing()\nadd_test(NAME value COMMAND probe [=[two words'$]=] 42)");
        assert!(result.enabled);
        assert_eq!(
            result.tests,
            vec![CMakeTest {
                name: "value".into(),
                executable: "probe".into(),
                args: vec!["two words'$".into(), "42".into()],
            }]
        );
        assert_eq!(result.unsupported, Vec::<String>::new());
    }

    #[test]
    fn unsupported_test_semantics_are_visible() {
        for source in [
            "add_test(old probe)",
            "add_test(NAME x COMMAND ${TOOL})",
            "add_test(NAME x COMMAND $<TARGET_FILE:probe>)",
            "add_test(NAME x COMMAND probe [=[$<CONFIG>]=])",
            "add_test(NAME x COMMAND probe WORKING_DIRECTORY here)",
            "if(OFF)\nadd_test(NAME x COMMAND probe)\nendif()",
            "function(t)\nadd_test(NAME x COMMAND probe)\nendfunction()",
            "set_tests_properties(x PROPERTIES WILL_FAIL TRUE)",
            "set_property(TEST x PROPERTY DISABLED TRUE)",
        ] {
            assert!(!testing(source).unsupported.is_empty(), "{source}");
        }
    }
}
