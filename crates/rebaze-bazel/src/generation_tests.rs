#![allow(clippy::unwrap_used)]

use starlark_syntax::syntax::{AstModule, Dialect};

use crate::discovery::{ProbeOutcome, UnavailableReason};
use crate::{DiscoverySnapshot, MigrationConfig, generate, generate_from_cmake_with_config};

fn unavailable(project: &rebaze_cmake::CMakeProject) -> DiscoverySnapshot {
    let mut snapshot = DiscoverySnapshot::default();
    for module in &project.pkg_config_modules {
        snapshot
            .record(
                module,
                ProbeOutcome::Unavailable(UnavailableReason::ToolNotFound),
            )
            .unwrap();
    }
    snapshot
}

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
    let files = generate_from_cmake_with_config(
        &project,
        &MigrationConfig::default(),
        &unavailable(&project),
    )
    .unwrap();
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
    let files = generate_from_cmake_with_config(&project, &config, &unavailable(&project)).unwrap();
    assert_syntax(&files);
    let build = &files["BUILD.bazel"];
    assert!(build.contains("company_library("));
    assert!(build.contains("company_binary("));
    assert!(!build.contains("\ncc_library("));
    config.build.rules.library_rule = "library)\nfail('injected')".into();
    assert!(generate_from_cmake_with_config(&project, &config, &unavailable(&project)).is_err());
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
    let files = generate_from_cmake_with_config(
        &project,
        &MigrationConfig::default(),
        &unavailable(&project),
    )
    .unwrap();
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
    let files = generate_from_cmake_with_config(&project, &config, &unavailable(&project)).unwrap();
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

fn observed_project() -> rebaze_cmake::CMakeProject {
    rebaze_cmake::CMakeProject {
        name: "snapshot_example".into(),
        pkg_config_modules: vec![rebaze_cmake::PkgConfigModule {
            prefix: "MY-LIB".into(),
            packages: vec!["my-lib>=1.0".into()],
            required: true,
        }],
        ..rebaze_cmake::CMakeProject::default()
    }
}

#[test]
fn snapshot_is_the_complete_host_input_and_round_trips() {
    use crate::discovery::{PackageInfo, SystemLibrary};
    let project = observed_project();
    let mut snapshot = DiscoverySnapshot::default();
    snapshot
        .record(
            &project.pkg_config_modules[0],
            ProbeOutcome::Available(PackageInfo {
                cflags: vec!["-I/sdk with spaces/include".into(), "-DCUSTOM=1".into()],
                linkopts: vec!["-L/sdk with spaces/lib".into(), "-lcustom".into()],
                system: Some(SystemLibrary {
                    prefix: "/sdk with spaces".into(),
                    include_dirs: vec!["include".into()],
                    lib_dirs: vec!["lib".into()],
                    libs: vec!["custom".into()],
                }),
            }),
        )
        .unwrap();
    let serialized = toml::to_string(&snapshot).unwrap();
    let restored: DiscoverySnapshot = toml::from_str(&serialized).unwrap();
    assert_eq!(snapshot, restored);
    let config = MigrationConfig::default();
    let expected = generate_from_cmake_with_config(&project, &config, &snapshot).unwrap();
    assert_syntax(&expected);
    assert!(expected["third_party/system_deps.bzl"].contains("/sdk with spaces"));
    assert!(expected["third_party/BUILD.bazel"].contains("-lcustom"));
    assert!(!expected["third_party/BUILD.bazel"].contains("copts ="));
    for _ in 0..4 {
        assert_eq!(
            expected,
            generate_from_cmake_with_config(&project, &config, &restored).unwrap()
        );
    }
    // Unrequested observations cannot leak host variation into generated output.
    snapshot
        .record(
            &rebaze_cmake::PkgConfigModule {
                prefix: "UNRELATED".into(),
                packages: vec!["unrelated".into()],
                required: false,
            },
            ProbeOutcome::Unavailable(UnavailableReason::ToolNotFound),
        )
        .unwrap();
    assert_eq!(
        expected,
        generate_from_cmake_with_config(&project, &config, &snapshot).unwrap()
    );
}

#[test]
fn missing_mismatched_and_failed_observations_are_explicit_errors() {
    use crate::discovery::{ProbeFailure, SnapshotError};
    let project = observed_project();
    let config = MigrationConfig::default();
    assert!(matches!(
        generate_from_cmake_with_config(&project, &config, &DiscoverySnapshot::default()),
        Err(crate::GenerationError::Discovery(
            SnapshotError::Missing { .. }
        ))
    ));
    let mut snapshot = unavailable(&project);
    let mut changed_request = project.pkg_config_modules[0].clone();
    changed_request.packages = vec!["different".into()];
    assert!(matches!(
        snapshot.record(
            &changed_request,
            ProbeOutcome::Failed(ProbeFailure::Timeout)
        ),
        Err(SnapshotError::RequestMismatch { .. })
    ));
    let mut changed_project = project.clone();
    changed_project.pkg_config_modules[0] = changed_request;
    assert!(matches!(
        generate_from_cmake_with_config(&changed_project, &config, &snapshot),
        Err(crate::GenerationError::Discovery(
            SnapshotError::RequestMismatch { .. }
        ))
    ));
    snapshot
        .record(
            &project.pkg_config_modules[0],
            ProbeOutcome::Failed(ProbeFailure::Timeout),
        )
        .unwrap();
    assert!(matches!(
        generate_from_cmake_with_config(&project, &config, &snapshot),
        Err(crate::GenerationError::Discovery(SnapshotError::Failed {
            failure: ProbeFailure::Timeout,
            ..
        }))
    ));
}

#[test]
fn explicit_unavailability_renders_fallback_without_disguising_failure() {
    let project = observed_project();
    let files = generate_from_cmake_with_config(
        &project,
        &MigrationConfig::default(),
        &unavailable(&project),
    )
    .unwrap();
    assert_syntax(&files);
    assert!(files["third_party/BUILD.bazel"].contains("TODO: Review flags"));
    assert!(files["third_party/BUILD.bazel"].contains("-lmy"));
    assert!(files["third_party/system_deps.bzl"].contains("\"my_lib\": {}"));
}

struct RejectRenderEvents;

impl tracing::Subscriber for RejectRenderEvents {
    fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
        true
    }
    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        panic!("rendering must not emit spans")
    }
    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
    fn event(&self, _: &tracing::Event<'_>) {
        panic!("rendering must not emit log events")
    }
    fn enter(&self, _: &tracing::span::Id) {}
    fn exit(&self, _: &tracing::span::Id) {}
}

#[test]
fn rendering_returns_diagnostics_as_data_without_logging_or_config_injection() {
    tracing::subscriber::with_default(RejectRenderEvents, || {
        for language in ["C", "CXX"] {
            let malicious_standard = "unknown\r\nbuild --run_under=untrusted";
            let project = rebaze_cmake::CMakeProject {
                name: "example".into(),
                languages: vec![language.into()],
                c_standard: Some(malicious_standard.into()),
                cxx_standard: Some(malicious_standard.into()),
                ..Default::default()
            };
            let files =
                crate::generate_from_cmake(&project, &DiscoverySnapshot::default()).unwrap();
            assert!(files[".bazelrc"].contains("# WARNING: Unknown"));
            assert!(files[".bazelrc"].contains("unknown\\r\\nbuild"));
            assert!(!files[".bazelrc"].contains("\nbuild --run_under=untrusted"));
        }
        generate(&rebaze_gradle::GradleProject {
            name: "example".into(),
            path: ".".into(),
            subprojects: vec![],
            plugins: vec![],
            dependencies: vec![],
        })
        .unwrap();
    });
}
