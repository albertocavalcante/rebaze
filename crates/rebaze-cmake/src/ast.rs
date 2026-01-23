//! CMake Abstract Syntax Tree types.
//!
//! CMake files are a sequence of commands. Each command has a name and
//! a list of arguments. Arguments can be quoted, unquoted, or bracket-quoted,
//! and may contain variable references.

use serde::{Deserialize, Serialize};
use std::ops::Range;

/// Source span for error reporting.
pub type Span = Range<usize>;

/// A complete CMake file (CMakeLists.txt or .cmake).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CMakeFile {
    pub commands: Vec<Command>,
}

/// A CMake command invocation.
///
/// Examples:
/// - `project(myapp VERSION 1.0.0)`
/// - `add_executable(myapp main.cpp)`
/// - `if(CONDITION)`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Command {
    /// Command name (normalized to lowercase for comparison).
    pub name: String,
    /// Original command name as written.
    pub name_original: String,
    /// Command arguments.
    pub arguments: Vec<Argument>,
    /// Source span for error reporting.
    #[serde(skip)]
    pub span: Span,
}

/// A command argument.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum Argument {
    /// Unquoted argument: `foo`, `${VAR}`, `path/to/file`
    Unquoted(ArgumentValue),
    /// Double-quoted argument: `"hello world"`
    Quoted(ArgumentValue),
    /// Bracket-quoted argument: `[[raw content]]` or `[=[raw]=]`
    Bracket(String),
}

/// The value of an argument, which may contain variable references.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ArgumentValue {
    pub parts: Vec<ArgumentPart>,
}

/// A part of an argument value.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum ArgumentPart {
    /// Literal text.
    Text(String),
    /// Variable reference: `${VAR_NAME}`
    Variable(String),
    /// Environment variable: `$ENV{VAR_NAME}`
    EnvVariable(String),
    /// Cache variable: `$CACHE{VAR_NAME}`
    CacheVariable(String),
    /// Generator expression: `$<EXPR:...>`
    GeneratorExpr(String),
}

impl Argument {
    /// Get the argument as a simple string, expanding nothing.
    /// Returns None if the argument contains variables or generator expressions.
    #[must_use]
    pub fn as_literal(&self) -> Option<&str> {
        match self {
            Self::Bracket(s) => Some(s),
            Self::Quoted(v) | Self::Unquoted(v) => {
                if v.parts.len() == 1 {
                    if let ArgumentPart::Text(s) = &v.parts[0] {
                        return Some(s);
                    }
                }
                None
            }
        }
    }

    /// Get the argument as a string, ignoring variable references.
    /// Useful for extracting string literals even if mixed with variables.
    #[must_use]
    pub fn to_string_lossy(&self) -> String {
        match self {
            Self::Bracket(s) => s.clone(),
            Self::Quoted(v) | Self::Unquoted(v) => v
                .parts
                .iter()
                .filter_map(|p| {
                    if let ArgumentPart::Text(s) = p {
                        Some(s.as_str())
                    } else {
                        None
                    }
                })
                .collect(),
        }
    }
}

impl ArgumentValue {
    /// Create a simple text-only argument value.
    #[must_use]
    pub fn text(s: impl Into<String>) -> Self {
        Self {
            parts: vec![ArgumentPart::Text(s.into())],
        }
    }

    /// Check if this value is just a simple text literal.
    #[must_use]
    pub fn is_simple(&self) -> bool {
        self.parts.len() == 1 && matches!(&self.parts[0], ArgumentPart::Text(_))
    }
}

impl Command {
    /// Check if this command matches the given name (case-insensitive).
    #[must_use]
    pub fn is(&self, name: &str) -> bool {
        self.name.eq_ignore_ascii_case(name)
    }

    /// Get an argument at the given index as a literal string.
    #[must_use]
    pub fn arg_literal(&self, index: usize) -> Option<&str> {
        self.arguments.get(index).and_then(Argument::as_literal)
    }

    /// Get all arguments as literal strings (skipping any with variables).
    #[must_use]
    pub fn args_literals(&self) -> Vec<&str> {
        self.arguments.iter().filter_map(Argument::as_literal).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_argument_as_literal() {
        let arg = Argument::Unquoted(ArgumentValue::text("hello"));
        assert_eq!(arg.as_literal(), Some("hello"));

        let arg_with_var = Argument::Unquoted(ArgumentValue {
            parts: vec![
                ArgumentPart::Text("prefix_".to_string()),
                ArgumentPart::Variable("VAR".to_string()),
            ],
        });
        assert_eq!(arg_with_var.as_literal(), None);
        assert_eq!(arg_with_var.to_string_lossy(), "prefix_");
    }

    #[test]
    fn test_command_is() {
        let cmd = Command {
            name: "project".to_string(),
            name_original: "PROJECT".to_string(),
            arguments: vec![],
            span: 0..7,
        };
        assert!(cmd.is("project"));
        assert!(cmd.is("PROJECT"));
        assert!(cmd.is("Project"));
    }
}
