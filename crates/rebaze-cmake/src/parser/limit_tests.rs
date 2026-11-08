#![allow(clippy::unwrap_used)]

use std::process::Command;

use super::limits::{MAX_NESTING, MAX_SOURCE_BYTES};
use crate::{ParseLimits, parse_source, parse_source_with_limits};

fn source(kind: &str, scopes: usize) -> String {
    let variable = |scopes: usize| {
        let references = scopes * 2 + 1;
        format!("{}leaf{}", "${".repeat(references), "}".repeat(references))
    };
    match kind {
        "parenthesis" => format!(
            "message(STATUS oracle {}leaf{})",
            "(".repeat(scopes),
            ")".repeat(scopes)
        ),
        "variable" => format!("message(STATUS oracle {})", variable(scopes)),
        "quoted-variable" => format!("message(STATUS oracle \"{}\")", variable(scopes)),
        "embedded-variable" => format!("message(STATUS oracle -D=\"{}\")", variable(scopes)),
        "embedded-generator" => format!(
            "message(STATUS oracle -D=\"$<{}leaf{}>\")",
            "<".repeat(scopes),
            ">".repeat(scopes)
        ),
        "generator" => format!(
            "message(STATUS oracle $<{}leaf{}>)",
            "<".repeat(scopes),
            ">".repeat(scopes)
        ),
        "mixed" => {
            let parentheses = scopes / 2;
            format!(
                "message(STATUS oracle {}{}{})",
                "(".repeat(parentheses),
                variable(scopes - parentheses),
                ")".repeat(parentheses)
            )
        }
        _ => panic!("unknown syntax family"),
    }
}

#[test]
fn recursive_families_obey_one_budget_in_fresh_processes() {
    // A stack overflow aborts the process and cannot be caught with catch_unwind.
    // Isolate even legitimate boundary cases so a broken limit reports a normal
    // assertion failure instead of taking down the complete test runner.
    for kind in [
        "parenthesis",
        "variable",
        "quoted-variable",
        "embedded-variable",
        "embedded-generator",
        "generator",
        "mixed",
    ] {
        for (scopes, selected) in [
            (MAX_NESTING - 1, MAX_NESTING),
            (MAX_NESTING, MAX_NESTING),
            (MAX_NESTING + 1, MAX_NESTING),
            (4096, MAX_NESTING),
            (128, 128),
            (129, 128),
        ] {
            let output = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--ignored",
                    "--exact",
                    "parser::limit_tests::syntax_limit_process_helper",
                ])
                .env("REBAZE_CMAKE_SYNTAX_FAMILY", kind)
                .env("REBAZE_CMAKE_SYNTAX_SCOPES", scopes.to_string())
                .env("REBAZE_CMAKE_SYNTAX_LIMIT", selected.to_string())
                .env("RUST_MIN_STACK", "2097152")
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{kind} / {scopes} / limit {selected}: {}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }
}

#[test]
#[ignore = "subprocess helper explicitly invoked by recursive_families_obey_one_budget_in_fresh_processes"]
fn syntax_limit_process_helper() {
    let kind = std::env::var("REBAZE_CMAKE_SYNTAX_FAMILY").unwrap();
    let scopes: usize = std::env::var("REBAZE_CMAKE_SYNTAX_SCOPES")
        .unwrap()
        .parse()
        .unwrap();
    let selected: usize = std::env::var("REBAZE_CMAKE_SYNTAX_LIMIT")
        .unwrap()
        .parse()
        .unwrap();
    let limits = ParseLimits::new(MAX_SOURCE_BYTES, selected).unwrap();
    let parsed = parse_source_with_limits(&source(&kind, scopes), limits);
    if scopes <= selected {
        assert!(parsed.is_ok(), "{parsed:?}");
    } else {
        let error = parsed.unwrap_err();
        assert!(error.to_string().contains("nesting budget"), "{error:#}");
    }
}

#[test]
fn custom_profiles_are_validated_and_allow_large_legitimate_source() {
    for invalid in [0, 4 * MAX_SOURCE_BYTES + 1, usize::MAX] {
        assert!(ParseLimits::new(invalid, MAX_NESTING).is_err());
    }
    for invalid in [0, 129, usize::MAX] {
        assert!(ParseLimits::new(MAX_SOURCE_BYTES, invalid).is_err());
    }
    let limits = ParseLimits::new(4 * MAX_SOURCE_BYTES, 128).unwrap();
    assert_eq!(limits.source_bytes(), 4 * MAX_SOURCE_BYTES);
    assert_eq!(limits.nesting(), 128);
    let source = format!("#{}", "x".repeat(limits.source_bytes() - 1));
    assert!(parse_source(&source).is_err());
    assert!(parse_source_with_limits(&source, limits).is_ok());
    assert!(parse_source_with_limits(&(source + "x"), limits).is_err());
}

#[test]
fn source_byte_limit_accepts_the_boundary_and_rejects_the_next_byte() {
    let valid = format!("#{}", "x".repeat(MAX_SOURCE_BYTES - 1));
    assert!(parse_source(&valid).is_ok());
    let oversized = format!("{valid}x");
    assert!(
        parse_source(&oversized)
            .unwrap_err()
            .to_string()
            .contains("source byte budget")
    );
    // Count UTF-8 bytes, not characters; there is no parser allocation for an
    // already over-budget borrowed source.
    let unicode = format!("#{}", "é".repeat(MAX_SOURCE_BYTES / 2));
    assert!(
        parse_source(&unicode)
            .unwrap_err()
            .to_string()
            .contains("source byte budget")
    );
}

#[test]
fn literal_delimiters_do_not_consume_recursive_scopes() {
    let literal = format!(
        "{}{}{}",
        "(".repeat(4096),
        "${".repeat(4096),
        "<".repeat(4096)
    );
    for source in [
        format!("# {literal}\nmessage(ok)"),
        format!("#[==[{literal}]==]\nmessage(ok)"),
        format!("message([==[{literal}]==])"),
        format!("message(\"{}\")", "(".repeat(4096)),
        format!("message(\"{}\")", "$<".repeat(4096)),
    ] {
        assert!(
            parse_source(&source).is_ok(),
            "literal input should not hit recursion limits"
        );
    }
}

#[test]
fn completed_and_failed_branches_release_their_scopes() {
    let source = format!("message({})", "(x) ".repeat(10_000));
    let parsed = parse_source(&source).unwrap();
    assert_eq!(parsed.commands[0].arguments.len(), 10_000);
    assert!(parse_source("message((unfinished)").is_err());
    assert!(parse_source("message((valid))").is_ok());
    // Each call owns its budget, including after a previous exhausted parse.
    assert!(parse_source(&self::source("parenthesis", MAX_NESTING + 1)).is_err());
    assert!(parse_source("message((valid))").is_ok());
}
