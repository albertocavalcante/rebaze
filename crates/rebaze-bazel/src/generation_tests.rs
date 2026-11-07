#![allow(clippy::unwrap_used)]

use starlark_syntax::syntax::{AstModule, Dialect};

use crate::{MigrationConfig, generate, generate_from_cmake_with_config};

fn assert_syntax(files: &std::collections::HashMap<String, String>) {
    for (path, source) in files {
        if matches!(
            std::path::Path::new(path)
                .extension()
                .and_then(|extension| extension.to_str()),
            Some("bzl" | "bazel")
        ) {
            AstModule::parse(path, source.clone(), &Dialect::Standard)
                .unwrap_or_else(|error| panic!("{path}: {error}\n{source}"));
        }
    }
}

#[test]
fn gradle_project_and_target_strings_cannot_inject_source() {
    let project = rebaze_gradle::GradleProject {
        name: "example\"\"\"\nfail('injected')\r\0🦀".into(),
        path: ".".into(),
        subprojects: vec!["app\"\nfail('injected')\r".into()],
        plugins: vec![],
        dependencies: vec![],
    };
    let files = generate(&project).unwrap();
    assert_syntax(&files);
    assert_eq!(files["MODULE.bazel"].matches("module(").count(), 1);
}

#[test]
fn cmake_and_all_third_party_files_escape_untrusted_data() {
    let file = rebaze_cmake::parse_source(
        "project(example)\nadd_library(core STATIC core.c)\nadd_executable(app main.c)",
    )
    .unwrap();
    let mut project = rebaze_cmake::extract_project(&file, ".".into());
    project.name = "example\"\"\"\nfail('injected')\r🦀".into();
    project.pkg_config_modules = vec![rebaze_cmake::PkgConfigModule {
        prefix: "unknown\"\nfail('injected')\r".into(),
        packages: vec!["rebaze-unavailable\"\\package\n# injected".into()],
        required: false,
    }];
    let files = generate_from_cmake_with_config(&project, &MigrationConfig::default()).unwrap();
    assert_syntax(&files);
    assert_eq!(files.len(), 8);
    let build = &files["third_party/BUILD.bazel"];
    // File composition hoists foreign_cc imports ahead of generated rule calls.
    assert!(build.find("@rules_foreign_cc").unwrap() < build.find("package(").unwrap());
}

#[test]
fn configured_custom_rules_are_used_and_invalid_names_fail_generation() {
    let file = rebaze_cmake::parse_source(
        "project(example)\nadd_library(core STATIC core.c)\nadd_executable(app main.c)",
    )
    .unwrap();
    let project = rebaze_cmake::extract_project(&file, ".".into());
    let mut config = MigrationConfig::default();
    config.build.rules.library_rule = "company_library".into();
    config.build.rules.binary_rule = "company_binary".into();
    config
        .build
        .rules
        .extra_loads
        .push(crate::config::LoadStatement {
            bzl: "//:company.bzl".into(),
            items: vec!["company_library".into(), "company_binary".into()],
        });
    let files = generate_from_cmake_with_config(&project, &config).unwrap();
    assert_syntax(&files);
    let build = &files["BUILD.bazel"];
    assert!(build.contains("company_library("));
    assert!(build.contains("company_binary("));
    assert!(!build.contains("\ncc_library("));
    config.build.rules.library_rule = "library)\nfail('injected')".into();
    assert!(generate_from_cmake_with_config(&project, &config).is_err());
}

#[test]
fn known_source_builds_include_their_transitive_dependencies() {
    let project = rebaze_cmake::CMakeProject {
        name: "example".into(),
        pkg_config_modules: vec![rebaze_cmake::PkgConfigModule {
            prefix: "gobject".into(),
            packages: vec!["gobject-2.0".into()],
            required: false,
        }],
        ..rebaze_cmake::CMakeProject::default()
    };
    let files = generate_from_cmake_with_config(&project, &MigrationConfig::default()).unwrap();
    assert_syntax(&files);
    for dependency in [
        "glib_source",
        "pcre2_source",
        "libffi_source",
        "zlib_source",
    ] {
        assert!(
            files["third_party/BUILD.bazel"].contains(dependency),
            "{dependency}"
        );
    }
}

#[test]
fn configured_source_metadata_and_meson_options_are_used_consistently() {
    let project = rebaze_cmake::CMakeProject {
        name: "example".into(),
        pkg_config_modules: vec![rebaze_cmake::PkgConfigModule {
            prefix: "glib".into(),
            packages: vec!["rebaze-unavailable-glib".into()],
            required: false,
        }],
        ..rebaze_cmake::CMakeProject::default()
    };
    let mut config = MigrationConfig::default();
    let glib = config.mappings.known_packages.get_mut("glib").unwrap();
    glib.url = "https://example.invalid/custom-glib.tar.xz".into();
    glib.build_options = vec!["-Dtests=false".into(), "-Dcustom_flag".into()];
    glib.deps = vec!["gobject".into()]; // A cycle must terminate.
    let custom = config.mappings.known_packages["zlib"].clone();
    config
        .mappings
        .known_packages
        .insert("custom_dep".into(), custom);
    config
        .mappings
        .transitive_deps
        .insert("glib".into(), vec!["custom_dep".into()]);
    let files = generate_from_cmake_with_config(&project, &config).unwrap();
    assert_syntax(&files);
    assert!(files["third_party/source.bzl"].contains("https://example.invalid/custom-glib.tar.xz"));
    let build = &files["third_party/BUILD.bazel"];
    assert!(build.contains("\"tests\": \"false\""));
    assert!(build.contains("\"custom_flag\": \"true\""));
    assert!(!build.contains("\"Dtests\""));
    assert!(files["MODULE.bazel"].contains("gobject_src"));
    assert!(files["MODULE.bazel"].contains("custom_dep_src"));
    assert!(build.contains(":custom_dep_source"));
    assert!(build.contains("name = \"custom_dep_source\""));
    assert_eq!(build.matches("name = \"glib_source\"").count(), 1);
    // Host-only -I flags must never leak into fallback cc_library rules.
    assert!(!build.contains("copts ="));
}
