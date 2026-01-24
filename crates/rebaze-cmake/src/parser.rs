//! CMake parser using chumsky.
//!
//! This parser handles CMakeLists.txt and .cmake files, extracting
//! commands with their arguments for analysis.

use chumsky::prelude::*;

use crate::ast::{Argument, ArgumentPart, ArgumentValue, CMakeFile, Command};

/// Parse a CMake file from source text.
///
/// # Errors
/// Returns parsing errors with source spans for error reporting.
pub fn parse(src: &str) -> (Option<CMakeFile>, Vec<Simple<char>>) {
    let (commands, errors) = cmake_file().parse_recovery(src);
    (commands.map(|commands| CMakeFile { commands }), errors)
}

/// Parser for a complete CMake file.
fn cmake_file() -> impl Parser<char, Vec<Command>, Error = Simple<char>> {
    trivia()
        .ignore_then(command().padded_by(trivia()).repeated())
        .then_ignore(trivia())
        .then_ignore(end())
}

/// Parser for trivia (whitespace and comments).
fn trivia() -> impl Parser<char, (), Error = Simple<char>> + Clone {
    let line_comment = just('#').then(none_of("\n\r").repeated()).ignored();

    let bracket_comment = just('#').ignore_then(bracket_content()).ignored();

    choice((bracket_comment, line_comment, one_of(" \t\n\r").ignored()))
        .repeated()
        .ignored()
}

/// Parser for separators between arguments (whitespace, newlines, semicolons, comments).
fn arg_separator() -> impl Parser<char, (), Error = Simple<char>> + Clone {
    // CMake allows any whitespace (including newlines), semicolons, and comments between arguments
    let ws_or_semi = one_of(" \t\n\r;").ignored();
    let line_comment = just('#').then(none_of("\n\r").repeated()).ignored();
    let bracket_comment = just('#').ignore_then(bracket_content()).ignored();

    choice((ws_or_semi, line_comment, bracket_comment))
        .repeated()
        .at_least(1)
        .ignored()
}

/// Parser for a CMake command.
fn command() -> impl Parser<char, Command, Error = Simple<char>> {
    let name = filter(|c: &char| c.is_ascii_alphabetic() || *c == '_')
        .then(filter(|c: &char| c.is_ascii_alphanumeric() || *c == '_').repeated())
        .map(|(first, rest): (char, Vec<char>)| {
            let mut s = String::with_capacity(1 + rest.len());
            s.push(first);
            s.extend(rest);
            s
        });

    name.map_with_span(|n, span| (n, span))
        .then_ignore(trivia())
        .then_ignore(just('('))
        .then_ignore(trivia())
        .then(argument_list())
        .then_ignore(trivia())
        .then_ignore(just(')'))
        .map_with_span(|((name_original, name_span), arguments), span| {
            let name = name_original.to_lowercase();
            Command {
                name,
                name_original,
                arguments,
                span: name_span.start..span.end,
            }
        })
}

/// Parser for a list of arguments.
fn argument_list() -> impl Parser<char, Vec<Argument>, Error = Simple<char>> {
    argument()
        .separated_by(arg_separator())
        .allow_leading()
        .allow_trailing()
        .or_not()
        .map(|args| args.unwrap_or_default())
}

/// Parser for a single argument (including nested parentheses with mixed arg types).
fn argument() -> impl Parser<char, Argument, Error = Simple<char>> {
    recursive(|arg| {
        // Parenthesis group that can contain any argument type
        let paren_group = just('(')
            .ignore_then(
                arg.clone()
                    .separated_by(arg_separator())
                    .allow_leading()
                    .allow_trailing()
                    .map(|args: Vec<Argument>| {
                        // Flatten all arguments into a single string with parens
                        let mut content = String::from("(");
                        for (i, a) in args.iter().enumerate() {
                            if i > 0 {
                                content.push(' ');
                            }
                            // Reconstruct quoted strings with quotes
                            match a {
                                Argument::Quoted(_) => {
                                    content.push('"');
                                    content.push_str(&a.to_string_with_vars());
                                    content.push('"');
                                }
                                _ => content.push_str(&a.to_string_with_vars()),
                            }
                        }
                        content.push(')');
                        Argument::Unquoted(ArgumentValue {
                            parts: vec![ArgumentPart::Text(content)],
                        })
                    }),
            )
            .then_ignore(just(')'));

        choice((
            bracket_argument(),
            quoted_argument(),
            paren_group,
            unquoted_argument_simple(),
        ))
    })
}

/// Parser for unquoted arguments (simple version without nested parens - those are handled at argument level).
fn unquoted_argument_simple() -> impl Parser<char, Argument, Error = Simple<char>> {
    // Escape sequences
    let escape_sequence = just('\\').ignore_then(any()).map(|c| vec![c]);

    // Regular characters (no whitespace, quotes, #, $, ;, or parens)
    let regular_chars = filter(|c: &char| {
        !c.is_whitespace() && !matches!(c, '(' | ')' | '#' | '"' | '\\' | '$' | ';')
    })
    .repeated()
    .at_least(1)
    .collect::<Vec<char>>();

    let text_part = choice((escape_sequence, regular_chars))
        .repeated()
        .at_least(1)
        .flatten()
        .collect::<String>()
        .map(ArgumentPart::Text);

    let var_ref = variable_reference();

    choice((var_ref, text_part))
        .repeated()
        .at_least(1)
        .map(|parts| Argument::Unquoted(ArgumentValue { parts }))
}

/// Parser for bracket-quoted arguments: [[content]] or [=[content]=]
fn bracket_argument() -> impl Parser<char, Argument, Error = Simple<char>> {
    bracket_content().map(Argument::Bracket)
}

/// Parser for bracket-quoted content: [[content]] or [=[content]=]
fn bracket_content() -> impl Parser<char, String, Error = Simple<char>> + Clone {
    just('[')
        .ignore_then(just('=').repeated().collect::<String>())
        .then_ignore(just('['))
        .then_with(move |equals: String| {
            // Build the closing pattern: ]===] where = count matches
            let close_pattern: String = format!("]{equals}]");
            take_until(just(close_pattern)).map(move |(chars, _): (Vec<char>, _)| {
                let content: String = chars.into_iter().collect();
                content
            })
        })
}

/// Parser for double-quoted arguments: "content"
fn quoted_argument() -> impl Parser<char, Argument, Error = Simple<char>> {
    just('"')
        .ignore_then(quoted_content())
        .then_ignore(just('"'))
        .map(|parts| Argument::Quoted(ArgumentValue { parts }))
}

/// Parser for content inside double quotes.
fn quoted_content() -> impl Parser<char, Vec<ArgumentPart>, Error = Simple<char>> {
    let escape_sequence = just('\\').ignore_then(choice((
        just('\\').to('\\'),
        just('"').to('"'),
        just('n').to('\n'),
        just('t').to('\t'),
        just('r').to('\r'),
        just(';').to(';'),
        just('$').to('$'),
    )));

    let regular_char = none_of("\"\\$");

    let text_char = escape_sequence.or(regular_char);

    let text = text_char
        .repeated()
        .at_least(1)
        .collect::<String>()
        .map(ArgumentPart::Text);

    let var_ref = variable_reference();

    choice((var_ref, text)).repeated()
}

/// Parser for variable references: ${VAR}, $ENV{VAR}, $CACHE{VAR}, $<GENEXPR>
/// Supports nested variables like ${${VARNAME}}
fn variable_reference() -> impl Parser<char, ArgumentPart, Error = Simple<char>> {
    recursive(|var_ref| {
        // Variable content can be: alphanumeric, underscore, or nested ${...}
        let plain_chars = filter(|c: &char| c.is_ascii_alphanumeric() || *c == '_')
            .repeated()
            .at_least(1)
            .collect::<String>();

        // Nested variable reference - capture as string including ${}
        let nested_var = just("${")
            .then(
                var_ref
                    .clone()
                    .map(|part: ArgumentPart| match part {
                        ArgumentPart::Variable(s) => format!("${{{s}}}"),
                        ArgumentPart::EnvVariable(s) => format!("$ENV{{{s}}}"),
                        ArgumentPart::CacheVariable(s) => format!("$CACHE{{{s}}}"),
                        ArgumentPart::GeneratorExpr(s) => format!("$<{s}>"),
                        ArgumentPart::Text(s) => s,
                    })
                    .or(plain_chars)
                    .repeated()
                    .at_least(1)
                    .collect::<Vec<String>>(),
            )
            .then_ignore(just('}'))
            .map(|(_, parts)| parts.join(""));

        // Content inside ${} can be plain chars or nested refs
        let var_content = plain_chars
            .or(nested_var.clone())
            .repeated()
            .at_least(1)
            .collect::<Vec<String>>()
            .map(|parts| parts.join(""));

        let normal_var = just("${")
            .ignore_then(var_content.clone())
            .then_ignore(just('}'))
            .map(ArgumentPart::Variable);

        let env_var = just("$ENV{")
            .ignore_then(var_content.clone())
            .then_ignore(just('}'))
            .map(ArgumentPart::EnvVariable);

        let cache_var = just("$CACHE{")
            .ignore_then(var_content)
            .then_ignore(just('}'))
            .map(ArgumentPart::CacheVariable);

        // Generator expressions: $<...> - handle nested <> properly
        let gen_expr = just("$<")
            .ignore_then(gen_expr_content())
            .then_ignore(just('>'))
            .map(ArgumentPart::GeneratorExpr);

        choice((env_var, cache_var, normal_var, gen_expr))
    })
}

/// Parser for generator expression content (handles nested <>)
fn gen_expr_content() -> impl Parser<char, String, Error = Simple<char>> {
    recursive(|content| {
        let nested = just('<')
            .then(content.clone())
            .then(just('>'))
            .map(|((open, inner), close): ((char, String), char)| format!("{open}{inner}{close}"));

        let plain = none_of("<>").map(|c: char| c.to_string());

        choice((nested, plain))
            .repeated()
            .collect::<Vec<String>>()
            .map(|parts: Vec<String>| parts.join(""))
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn parse_ok(src: &str) -> CMakeFile {
        let (result, errors) = parse(src);
        assert!(errors.is_empty(), "Parse errors: {errors:?}");
        result.unwrap_or_else(|| panic!("Parse failed"))
    }

    #[test]
    fn test_nested_parentheses() {
        // This is a common pattern in CMake if() conditions
        let file = parse_ok("if(${MAIN_PROJECT} AND (${CMAKE_VERSION} VERSION_EQUAL 3.13 OR ${CMAKE_VERSION} VERSION_GREATER 3.13))");
        assert_eq!(file.commands.len(), 1);
        assert!(file.commands[0].is("if"));
        // Should have parsed the nested parens as part of the arguments
        assert!(file.commands[0].arguments.len() >= 2);
    }

    #[test]
    fn test_deeply_nested_parentheses() {
        let file = parse_ok("if(A AND (B OR (C AND D)))");
        assert_eq!(file.commands.len(), 1);
        assert!(file.commands[0].is("if"));
    }

    #[test]
    fn test_nested_variable_reference() {
        // Common pattern: ${${VARNAME}}
        let file = parse_ok(r#"string(REPLACE " " ";" BUILD_FLAGS_AS_LIST "${${VARNAME}}")"#);
        assert_eq!(file.commands.len(), 1);
        assert!(file.commands[0].is("string"));
    }

    #[test]
    fn test_nested_gen_expr() {
        // Nested generator expressions: $<$<CONFIG:Debug>:value>
        let file = parse_ok(r#"target_compile_definitions(foo $<$<CONFIG:Debug>:DEBUG_MODE>)"#);
        assert_eq!(file.commands.len(), 1);
        assert!(file.commands[0].is("target_compile_definitions"));
    }

    #[test]
    fn test_regex_in_quoted_string() {
        // Regex patterns with square brackets in quoted strings
        let file = parse_ok(r#"if (NOT ("XX${flag}" MATCHES "XX-O[0123s]"))"#);
        assert_eq!(file.commands.len(), 1);
        assert!(file.commands[0].is("if"));
    }

    #[test]
    fn test_empty_argument_list() {
        let file = parse_ok("endif()");
        assert_eq!(file.commands.len(), 1);
        assert!(file.commands[0].is("endif"));
        assert!(file.commands[0].arguments.is_empty());
    }

    #[test]
    fn test_simple_command() {
        let file = parse_ok("project(myapp)");
        assert_eq!(file.commands.len(), 1);
        assert!(file.commands[0].is("project"));
        assert_eq!(file.commands[0].arg_literal(0), Some("myapp"));
    }

    #[test]
    fn test_command_case_insensitive() {
        let file = parse_ok("PROJECT(MyApp)");
        assert!(file.commands[0].is("project"));
        assert_eq!(file.commands[0].name_original, "PROJECT");
    }

    #[test]
    fn test_multiple_arguments() {
        let file = parse_ok("add_executable(myapp main.cpp util.cpp)");
        assert_eq!(file.commands.len(), 1);
        let args = file.commands[0].args_literals();
        assert_eq!(args, vec!["myapp", "main.cpp", "util.cpp"]);
    }

    #[test]
    fn test_quoted_argument() {
        let file = parse_ok(r#"message("Hello, World!")"#);
        let arg = &file.commands[0].arguments[0];
        assert_eq!(arg.to_string_lossy(), "Hello, World!");
    }

    #[test]
    fn test_variable_reference() {
        let file = parse_ok("set(VAR ${OTHER_VAR})");
        assert_eq!(file.commands.len(), 1);
        let args = &file.commands[0].arguments;
        assert_eq!(args.len(), 2);

        if let Argument::Unquoted(val) = &args[1] {
            assert_eq!(val.parts.len(), 1);
            assert!(matches!(&val.parts[0], ArgumentPart::Variable(v) if v == "OTHER_VAR"));
        } else {
            panic!("Expected unquoted argument");
        }
    }

    #[test]
    fn test_comments() {
        let file = parse_ok(
            "
            # This is a comment
            project(myapp)
            #[=[ Bracket comment with equals ]=]
            # Another comment
        ",
        );
        assert_eq!(file.commands.len(), 1);
    }

    #[test]
    fn test_comment_only_file() {
        // Example: https://github.com/ggerganov/ggwave/blob/master/examples/rp2040-rx/CMakeLists.txt
        let file = parse_ok(
            r#"
# rp2040-rx
"#,
        );
        assert!(file.commands.is_empty());
    }

    #[test]
    fn test_multiline() {
        let file = parse_ok(
            "
cmake_minimum_required(VERSION 3.20)
project(myapp VERSION 1.0.0)
add_executable(myapp
    main.cpp
    util.cpp
    helper.cpp
)
",
        );
        assert_eq!(file.commands.len(), 3);
        assert!(file.commands[0].is("cmake_minimum_required"));
        assert!(file.commands[1].is("project"));
        assert!(file.commands[2].is("add_executable"));
    }

    #[test]
    fn test_bracket_argument() {
        let file = parse_ok(r#"message([[Raw content with "quotes" and ${vars}]])"#);
        if let Argument::Bracket(content) = &file.commands[0].arguments[0] {
            assert!(content.contains("\"quotes\""));
            assert!(content.contains("${vars}"));
        } else {
            panic!("Expected bracket argument");
        }
    }

    #[test]
    fn test_generator_expression() {
        let file = parse_ok("target_include_directories(mylib PUBLIC $<BUILD_INTERFACE:${CMAKE_CURRENT_SOURCE_DIR}/include>)");
        assert_eq!(file.commands.len(), 1);
    }
}
