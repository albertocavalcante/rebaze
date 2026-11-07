#![allow(clippy::unwrap_used)]

use starlark_syntax::syntax::ast::{AstLiteral, AstStmt, ExprP, StmtP};
use starlark_syntax::syntax::module::AstModuleFields;
use starlark_syntax::syntax::{AstModule, Dialect};

use crate::{Call, Error, File, Identifier, Load, Module, Reference, UseExtension, UseRepo};

fn parse(source: &str) -> AstModule {
    AstModule::parse("generated.star", source.to_owned(), &Dialect::Standard).unwrap()
}

fn statements(ast: &AstModule) -> &[AstStmt] {
    match &ast.statement().node {
        StmtP::Statements(statements) => statements,
        _ => std::slice::from_ref(ast.statement()),
    }
}

#[test]
fn strings_round_trip_through_an_independent_parser_as_data() {
    for value in [
        "plain",
        "a\\b\"c",
        "\r\nload(\"evil\", \"x\")",
        "\0",
        "\x007",
        "🦀 日本語",
        "\u{2028}",
        "'''\"\"\"",
    ] {
        let mut file = File::new();
        file.assign(&Identifier::new("VALUE").unwrap(), &value)
            .unwrap();
        let ast = parse(&file.render().unwrap());
        let [statement] = statements(&ast) else {
            panic!("injected statement");
        };
        let StmtP::Assign(assignment) = &statement.node else {
            panic!("expected assignment");
        };
        let ExprP::Literal(AstLiteral::String(decoded)) = &assignment.rhs.node else {
            panic!("expected string");
        };
        assert_eq!(decoded.node, value);
    }
}

#[test]
fn loads_are_hoisted_and_combined_when_documents_are_composed() {
    let mut file = File::new();
    file.statement(&Module {
        name: "example".into(),
        version: "1.0".into(),
    })
    .unwrap();
    file.load(Load {
        bzl: "//:rules.bzl".into(),
        items: ["first".into()].into(),
    })
    .unwrap();
    let mut other = File::new();
    other
        .load(Load {
            bzl: "//:rules.bzl".into(),
            items: ["second".into()].into(),
        })
        .unwrap();
    file.append(other);
    let output = file.render().unwrap();
    let ast = parse(&output);
    assert!(matches!(statements(&ast)[0].node, StmtP::Load(_)));
    assert_eq!(statements(&ast).len(), 2);
    assert_eq!(output.matches("load(").count(), 1);
    assert!(output.contains("\"first\""));
    assert!(output.contains("\"second\""));
}

#[test]
fn extension_references_are_identifiers_not_quoted_strings() {
    let mut file = File::new();
    file.assign(
        &Identifier::new("maven").unwrap(),
        &UseExtension("@rules_jvm_external//:extensions.bzl", "maven"),
    )
    .unwrap();
    file.statement(&UseRepo {
        extension: Reference::new("maven").unwrap(),
        repositories: vec!["maven".into()],
    })
    .unwrap();
    let output = file.render().unwrap();
    parse(&output);
    assert!(output.contains("maven = use_extension("));
    assert!(output.contains("use_repo(\n    maven,"));
}

#[test]
fn invalid_names_and_serialization_errors_are_reported() {
    for name in [
        "",
        "9value",
        "a.b",
        "x = fail('injected')",
        "def",
        "while",
        "None",
    ] {
        assert!(Identifier::new(name).is_err(), "{name}");
    }
    assert!(Call::new("x)\nfail", ("value",)).is_err());
    assert!(Call::new("macro..rule", ("value",)).is_err());
    assert!(Reference::new("None").is_err());
    let mut file = File::new();
    assert!(matches!(file.statement(&'x'), Err(Error::Serialize(_))));
    assert!(
        file.load(Load {
            bzl: "//:rules.bzl".into(),
            items: ["bad-name".into()].into()
        })
        .is_err()
    );
}

#[test]
fn comment_and_docstring_input_cannot_introduce_statements() {
    let mut file = File::new();
    file.docstring("\"\"\"\nfail('injected')\n\0").unwrap();
    file.comment("description\rfail('injected')\nload('evil', 'x')\0");
    let ast = parse(&file.render().unwrap());
    assert_eq!(statements(&ast).len(), 1);
    assert!(matches!(statements(&ast)[0].node, StmtP::Expression(_)));
}

#[test]
fn commented_statements_can_be_uncommented_without_changing_source() {
    let mut file = File::new();
    file.commented_statement(&serde_starlark::Assignment::new(
        "source_deps",
        UseExtension("//third_party:source.bzl", "source_deps"),
    ))
    .unwrap();
    file.commented_statement(&UseRepo {
        extension: Reference::new("source_deps").unwrap(),
        repositories: vec!["quote\"and\\slash_src".into()],
    })
    .unwrap();
    let output = file.render().unwrap();
    let uncommented = output
        .lines()
        .map(|line| line.strip_prefix("# ").unwrap_or(line))
        .collect::<Vec<_>>()
        .join("\n");
    assert_eq!(statements(&parse(&uncommented)).len(), 2);
    assert!(uncommented.contains("source_deps = use_extension("));
}
