//! CMake variable evaluation.
//!
//! Evaluates CMake `set()` and `list()` commands to track variable state,
//! and expands `${VAR}` references in arguments.

use std::collections::HashMap;

use crate::ast::{Argument, ArgumentPart, ArgumentValue, CMakeFile, Command};

/// CMake evaluation context tracking variable state.
#[derive(Debug, Default, Clone)]
pub struct EvalContext {
    /// Variables and their values (CMake vars are lists of strings).
    variables: HashMap<String, Vec<String>>,
}

impl EvalContext {
    /// Create a new empty evaluation context.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set a variable to the given values.
    pub fn set(&mut self, name: &str, values: Vec<String>) {
        self.variables.insert(name.to_string(), values);
    }

    /// Append values to a variable (creates if doesn't exist).
    pub fn append(&mut self, name: &str, values: Vec<String>) {
        self.variables
            .entry(name.to_string())
            .or_default()
            .extend(values);
    }

    /// Get a variable's values.
    pub fn get(&self, name: &str) -> Option<&Vec<String>> {
        self.variables.get(name)
    }

    /// Expand `${VAR}` references in a string.
    ///
    /// Returns the expanded string. If a variable is not found, it expands to empty.
    pub fn expand(&self, value: &str) -> String {
        let mut result = String::new();
        let mut chars = value.chars().peekable();

        while let Some(c) = chars.next() {
            if c == '$' && chars.peek() == Some(&'{') {
                // Consume '{'
                chars.next();

                // Collect variable name until '}'
                let mut var_name = String::new();
                while let Some(&ch) = chars.peek() {
                    if ch == '}' {
                        chars.next();
                        break;
                    }
                    if let Some(c) = chars.next() {
                        var_name.push(c);
                    }
                }

                // Look up and expand
                if let Some(values) = self.variables.get(&var_name) {
                    result.push_str(&values.join(";"));
                }
                // If not found, expand to empty (CMake behavior)
            } else {
                result.push(c);
            }
        }

        result
    }

    /// Expand an Argument, returning expanded string(s).
    ///
    /// For arguments containing only a variable reference, returns all values.
    /// For arguments with mixed content, returns a single concatenated string.
    pub fn expand_argument(&self, arg: &Argument) -> Vec<String> {
        match arg {
            Argument::Bracket(s) => vec![s.clone()],
            Argument::Quoted(value) | Argument::Unquoted(value) => {
                self.expand_argument_value(value)
            }
        }
    }

    /// Expand an ArgumentValue, returning expanded string(s).
    fn expand_argument_value(&self, value: &ArgumentValue) -> Vec<String> {
        // Special case: single variable reference expands to list
        if value.parts.len() == 1 {
            if let ArgumentPart::Variable(var_name) = &value.parts[0] {
                return self
                    .variables
                    .get(var_name)
                    .cloned()
                    .unwrap_or_default();
            }
        }

        // General case: concatenate all parts into a single string
        let mut result = String::new();
        for part in &value.parts {
            match part {
                ArgumentPart::Text(s) => result.push_str(s),
                ArgumentPart::Variable(var_name) => {
                    if let Some(values) = self.variables.get(var_name) {
                        // Join list with semicolons (CMake list separator)
                        result.push_str(&values.join(";"));
                    }
                }
                ArgumentPart::EnvVariable(_)
                | ArgumentPart::CacheVariable(_)
                | ArgumentPart::GeneratorExpr(_) => {
                    // Not supported yet - skip
                }
            }
        }

        if result.is_empty() {
            vec![]
        } else {
            vec![result]
        }
    }
}

/// Evaluate a CMake file, processing `set()` and `list()` commands.
///
/// Updates the context with variable definitions.
pub fn evaluate(file: &CMakeFile, ctx: &mut EvalContext) {
    for cmd in &file.commands {
        match cmd.name.as_str() {
            "set" => eval_set(cmd, ctx),
            "list" => eval_list(cmd, ctx),
            _ => {}
        }
    }
}

/// Evaluate a `set()` command.
///
/// Syntax: `set(VAR value1 value2 ...)`
fn eval_set(cmd: &Command, ctx: &mut EvalContext) {
    if cmd.arguments.is_empty() {
        return;
    }

    // First argument is the variable name
    let var_name = if let Some(name) = cmd.arguments[0].as_literal() {
        name.to_string()
    } else {
        // Variable name might be a variable reference itself
        let mut expanded = ctx.expand_argument(&cmd.arguments[0]);
        if expanded.len() != 1 {
            return; // Can't determine variable name
        }
        expanded.remove(0)
    };

    // Check for PARENT_SCOPE, CACHE, etc. - skip these for now
    for arg in cmd.arguments.iter().skip(1) {
        if let Some(lit) = arg.as_literal() {
            if matches!(
                lit.to_uppercase().as_str(),
                "PARENT_SCOPE" | "CACHE" | "FORCE"
            ) {
                // Just process normally for now, ignore the modifier
                break;
            }
        }
    }

    // Collect values (skip modifiers)
    let mut values = Vec::new();
    for arg in cmd.arguments.iter().skip(1) {
        if let Some(lit) = arg.as_literal() {
            if matches!(
                lit.to_uppercase().as_str(),
                "PARENT_SCOPE" | "CACHE" | "FORCE" | "STRING" | "BOOL" | "PATH" | "FILEPATH"
                    | "INTERNAL"
            ) {
                continue;
            }
        }
        // Expand any variable references in the value
        let expanded = ctx.expand_argument(arg);
        values.extend(expanded);
    }

    ctx.set(&var_name, values);
}

/// Evaluate a `list()` command.
///
/// Syntax: `list(APPEND VAR value1 value2 ...)`
fn eval_list(cmd: &Command, ctx: &mut EvalContext) {
    if cmd.arguments.len() < 2 {
        return;
    }

    let operation = match cmd.arguments[0].as_literal() {
        Some(op) => op.to_uppercase(),
        None => return,
    };

    let var_name = match cmd.arguments[1].as_literal() {
        Some(name) => name.to_string(),
        None => return,
    };

    match operation.as_str() {
        "APPEND" => {
            // list(APPEND VAR value1 value2 ...)
            let mut values = Vec::new();
            for arg in cmd.arguments.iter().skip(2) {
                let expanded = ctx.expand_argument(arg);
                values.extend(expanded);
            }
            ctx.append(&var_name, values);
        }
        "PREPEND" => {
            // list(PREPEND VAR value1 value2 ...)
            let mut values = Vec::new();
            for arg in cmd.arguments.iter().skip(2) {
                let expanded = ctx.expand_argument(arg);
                values.extend(expanded);
            }
            // Prepend to existing
            if let Some(existing) = ctx.variables.get(&var_name).cloned() {
                values.extend(existing);
            }
            ctx.set(&var_name, values);
        }
        // Other operations (LENGTH, GET, FIND, etc.) - not implemented yet
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_set_and_expand() {
        let mut ctx = EvalContext::new();
        ctx.set("FOO", vec!["a.c".into(), "b.c".into()]);
        assert_eq!(ctx.get("FOO"), Some(&vec!["a.c".into(), "b.c".into()]));
    }

    #[test]
    fn test_list_append() {
        let mut ctx = EvalContext::new();
        ctx.set("FOO", vec!["a.c".into()]);
        ctx.append("FOO", vec!["b.c".into()]);
        assert_eq!(ctx.get("FOO"), Some(&vec!["a.c".into(), "b.c".into()]));
    }

    #[test]
    fn test_expand_string() {
        let mut ctx = EvalContext::new();
        ctx.set("NAME", vec!["myapp".into()]);
        ctx.set("VERSION", vec!["1.0".into()]);

        assert_eq!(ctx.expand("${NAME}"), "myapp");
        assert_eq!(ctx.expand("${NAME}-${VERSION}"), "myapp-1.0");
        assert_eq!(ctx.expand("prefix_${NAME}_suffix"), "prefix_myapp_suffix");
        assert_eq!(ctx.expand("no_vars_here"), "no_vars_here");
        assert_eq!(ctx.expand("${UNDEFINED}"), "");
    }

    #[test]
    fn test_expand_list_variable() {
        let mut ctx = EvalContext::new();
        ctx.set("SRCS", vec!["a.c".into(), "b.c".into(), "c.c".into()]);

        // When expanding in a string context, lists are joined with semicolons
        assert_eq!(ctx.expand("${SRCS}"), "a.c;b.c;c.c");
    }

    #[test]
    fn test_expand_argument_single_var() {
        let mut ctx = EvalContext::new();
        ctx.set("SRCS", vec!["a.c".into(), "b.c".into()]);

        // Single variable reference should expand to multiple values
        let arg = Argument::Unquoted(ArgumentValue {
            parts: vec![ArgumentPart::Variable("SRCS".into())],
        });

        let expanded = ctx.expand_argument(&arg);
        assert_eq!(expanded, vec!["a.c", "b.c"]);
    }

    #[test]
    fn test_expand_argument_mixed() {
        let mut ctx = EvalContext::new();
        ctx.set("DIR", vec!["src".into()]);

        // Mixed text and variable should produce single concatenated result
        let arg = Argument::Unquoted(ArgumentValue {
            parts: vec![
                ArgumentPart::Variable("DIR".into()),
                ArgumentPart::Text("/main.cpp".into()),
            ],
        });

        let expanded = ctx.expand_argument(&arg);
        assert_eq!(expanded, vec!["src/main.cpp"]);
    }

    #[test]
    fn test_expand_argument_literal() {
        let ctx = EvalContext::new();

        let arg = Argument::Unquoted(ArgumentValue::text("hello.cpp"));
        let expanded = ctx.expand_argument(&arg);
        assert_eq!(expanded, vec!["hello.cpp"]);
    }

    #[test]
    fn test_eval_set_command() {
        use crate::parser;

        let src = r#"
            set(MY_VAR value1 value2 value3)
        "#;

        let (file, errors) = parser::parse(src);
        assert!(errors.is_empty());
        let file = file.unwrap();

        let mut ctx = EvalContext::new();
        evaluate(&file, &mut ctx);

        assert_eq!(
            ctx.get("MY_VAR"),
            Some(&vec!["value1".into(), "value2".into(), "value3".into()])
        );
    }

    #[test]
    fn test_eval_list_append() {
        use crate::parser;

        let src = r#"
            set(SRCS a.c)
            list(APPEND SRCS b.c c.c)
        "#;

        let (file, errors) = parser::parse(src);
        assert!(errors.is_empty());
        let file = file.unwrap();

        let mut ctx = EvalContext::new();
        evaluate(&file, &mut ctx);

        assert_eq!(
            ctx.get("SRCS"),
            Some(&vec!["a.c".into(), "b.c".into(), "c.c".into()])
        );
    }

    #[test]
    fn test_eval_nested_variable() {
        use crate::parser;

        let src = r#"
            set(PART1 a.c b.c)
            set(PART2 c.c)
            set(ALL ${PART1} ${PART2})
        "#;

        let (file, errors) = parser::parse(src);
        assert!(errors.is_empty());
        let file = file.unwrap();

        let mut ctx = EvalContext::new();
        evaluate(&file, &mut ctx);

        assert_eq!(
            ctx.get("ALL"),
            Some(&vec!["a.c".into(), "b.c".into(), "c.c".into()])
        );
    }
}
