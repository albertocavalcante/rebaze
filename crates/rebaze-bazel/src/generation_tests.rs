#![allow(clippy::unwrap_used)]

use starlark_syntax::syntax::{AstModule, Dialect};

use crate::discovery::{ProbeOutcome, UnavailableReason};
use crate::{DiscoverySnapshot, MigrationConfig, generate, generate_from_cmake_with_config};

fn java_module(path: &str) -> rebaze_gradle::GradleModule {
    rebaze_gradle::GradleModule {
        path: path.into(),
        plugins: vec![rebaze_gradle::GradlePlugin {
            id: "java".into(),
            version: None,
        }],
        ..rebaze_gradle::GradleModule::default()
    }
}

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
fn unresolved_native_links_fail_before_discovery() {
    for declaration in [
        "add_library(consumer consumer.c)",
        "add_executable(consumer consumer.c)",
    ] {
        for dependency in [
            "Unknown::Required",
            "unknown_required",
            "-lunknown_required",
        ] {
            let file = rebaze_cmake::parse_source(&format!(
                "project(example C)\n{declaration}\ntarget_link_libraries(consumer {dependency})"
            ))
            .unwrap();
            let project = rebaze_cmake::extract_project(&file, ".".into());
            let config = MigrationConfig::default();
            assert!(
                crate::plan_cmake(&project, &config).is_err(),
                "planning silently accepted {dependency:?} for {declaration}"
            );
            let error = crate::plan_cmake(&project, &config).unwrap_err();
            assert!(matches!(
                error,
                crate::GenerationError::NativeLink(crate::NativeLinkError::Unresolved {
                    target,
                    dependency: failed,
                }) if target == "consumer" && failed == dependency
            ));
        }
    }
}

#[test]
fn configured_native_links_and_explicit_omissions_remain_supported() {
    let file = rebaze_cmake::parse_source(
        "project(example C)\nadd_executable(consumer consumer.c)\ntarget_link_libraries(consumer custom -lcustom_system ignored empty pthread Threads::Threads fmt)",
    ).unwrap();
    let project = rebaze_cmake::extract_project(&file, ".".into());
    let mut config = MigrationConfig::default();
    config
        .mappings
        .plain_libraries
        .insert("custom".into(), "//deps:custom".into());
    config
        .mappings
        .system_libraries
        .insert("custom_system".into(), "//deps:system".into());
    config
        .mappings
        .plain_libraries
        .insert("empty".into(), String::new());
    config.mappings.ignored_libraries.push("ignored".into());
    let files =
        generate_from_cmake_with_config(&project, &config, &DiscoverySnapshot::default()).unwrap();
    assert_syntax(&files);
    let build = &files["BUILD.bazel"];
    assert!(build.contains("\"//deps:custom\""));
    assert!(build.contains("\"//deps:system\""));
    for omitted in ["ignored", "empty", "pthread", "Threads::Threads", "fmt"] {
        assert!(!build.contains(&format!("\"{omitted}\"")));
    }
    config
        .mappings
        .plain_libraries
        .insert("custom".into(), "# TODO: map custom".into());
    assert!(matches!(
        crate::plan_cmake(&project, &config),
        Err(crate::GenerationError::NativeLink(
            crate::NativeLinkError::CommentMapping { .. }
        ))
    ));
}

#[test]
fn cmake_tests_render_typed_rules_and_preserve_executables() {
    let file = rebaze_cmake::parse_source(
        "project(example C)\nenable_testing()\nadd_executable(probe main.c)\nadd_test(NAME value COMMAND probe [=[two words'$]=] 42)",
    ).unwrap();
    let project = rebaze_cmake::extract_project(&file, ".".into());
    let mut config = MigrationConfig::default();
    config.build.rules.test_rule = "company_test".into();
    let files =
        generate_from_cmake_with_config(&project, &config, &DiscoverySnapshot::default()).unwrap();
    assert_syntax(&files);
    let build = &files["BUILD.bazel"];
    assert!(build.contains("cc_binary("));
    assert!(build.contains("company_test("));
    assert!(build.contains("name = \"ctest_value\""));
    config.build.rules.test_rule = "invalid-test-rule".into();
    assert!(
        generate_from_cmake_with_config(&project, &config, &DiscoverySnapshot::default()).is_err()
    );
}

#[test]
fn unsupported_or_conflicting_cmake_tests_fail_during_planning() {
    let config = MigrationConfig::default();
    for (source, diagnostic) in [
        ("add_test(NAME x COMMAND probe)", "enable_testing"),
        (
            "enable_testing()\nadd_test(NAME x COMMAND missing)",
            "known executable",
        ),
        (
            "enable_testing()\nadd_test(NAME bad/name COMMAND probe)",
            "test name",
        ),
        (
            "enable_testing()\nadd_test(NAME x COMMAND probe)\nadd_test(NAME x COMMAND probe)",
            "collides",
        ),
        (
            "enable_testing()\nadd_test(NAME x-y COMMAND probe)\nadd_test(NAME x_y COMMAND probe)",
            "collides",
        ),
        (
            "enable_testing()\nadd_executable(ctest_x other.c)\nadd_test(NAME x COMMAND probe)",
            "collides",
        ),
        (
            "enable_testing()\nadd_test(NAME x COMMAND probe)\nset_tests_properties(x PROPERTIES WILL_FAIL TRUE)",
            "set_tests_properties",
        ),
        (
            "enable_testing()\nif(OFF)\nadd_test(NAME x COMMAND probe)\nendif()",
            "conditional",
        ),
        (
            "enable_testing()\nset_property(TARGET probe PROPERTY TEST_LAUNCHER /usr/bin/false)\nadd_test(NAME x COMMAND probe)",
            "launchers",
        ),
        (
            "enable_testing()\nset_target_properties(probe PROPERTIES CROSSCOMPILING_EMULATOR tool)\nadd_test(NAME x COMMAND probe)",
            "emulators",
        ),
        (
            "enable_testing()\nset(CMAKE_TEST_LAUNCHER tool)\nadd_test(NAME x COMMAND probe)",
            "initializers",
        ),
        (
            "enable_testing()\nset(PROP TEST_LAUNCHER)\nset_property(TARGET probe PROPERTY ${PROP} /usr/bin/false)\nadd_test(NAME x COMMAND probe)",
            "dynamic property",
        ),
        (
            "enable_testing()\nif(OFF)\nadd_subdirectory(tests)\nendif()\nadd_test(NAME x COMMAND probe)",
            "conditional or scoped add_subdirectory",
        ),
    ] {
        let file = rebaze_cmake::parse_source(&format!(
            "project(example C)\nadd_executable(probe main.c)\n{source}"
        ))
        .unwrap();
        let project = rebaze_cmake::extract_project(&file, ".".into());
        let error = crate::plan_cmake(&project, &config).unwrap_err();
        assert!(
            matches!(error, crate::GenerationError::Test(_)),
            "{source}: {error}"
        );
        assert!(error.to_string().contains(diagnostic), "{source}: {error}");
    }
}

#[test]
fn gradle_project_and_target_strings_cannot_inject_source() {
    let project = rebaze_gradle::GradleProject {
        name: "example\"\"\"\nfail('injected')\r\0🦀".into(),
        path: ".".into(),
        modules: vec![java_module("app")],
        maven_lock: None,
    };
    let files = generate(&project).unwrap();
    assert_syntax(&files);
    assert_eq!(files["MODULE.bazel"].matches("module(").count(), 1);
    let mut invalid = project;
    invalid.modules[0].path = "../outside".into();
    assert!(generate(&invalid).is_err());
}

fn java_project() -> rebaze_gradle::GradleProject {
    let mut core = rebaze_gradle::parse_build_source("plugins { id 'java-library' } repositories { mavenCentral() } dependencies { api 'org.hamcrest:hamcrest-core:1.3' }").unwrap();
    core.path = ":core".into();
    core.sources = vec!["src/main/java/example/Core.java".into()];
    let mut app = rebaze_gradle::parse_build_source("plugins { id 'java' } repositories { mavenCentral() } dependencies { implementation project(':core') runtimeOnly 'org.apache.commons:commons-lang3:3.17.0' testImplementation 'junit:junit:4.13.2' }").unwrap();
    app.path = ":app".into();
    app.sources = vec!["src/main/java/example/App.java".into()];
    app.test_sources = vec!["src/test/java/example/Behavior.java".into()];
    app.test_classes = vec!["example.Behavior".into()];
    rebaze_gradle::GradleProject {
        name: "example".into(),
        path: ".".into(),
        modules: vec![core, app],
        maven_lock: None,
    }
}

#[test]
fn java_generation_retains_project_api_runtime_and_test_scopes() {
    let project = java_project();
    let files = generate(&project).unwrap();
    assert_syntax(&files);
    let app = &files["app/BUILD.bazel"];
    let production = app.split("java_test(").next().unwrap();
    assert!(production.contains("//core:lib"));
    assert!(production.contains("deps = [\"//core:lib\"]"));
    assert!(
        production
            .contains("runtime_deps = [\"@rebaze_maven//:org_apache_commons_commons_lang3\"]")
    );
    assert!(!production.contains("junit_junit"));
    assert!(
        app.split("java_test(")
            .nth(1)
            .unwrap()
            .contains("@rebaze_maven//:junit_junit")
    );
    assert!(app.contains("test_class = \"example.Behavior\""));
    assert!(!app.contains("TODO"));
    assert!(
        files["core/BUILD.bazel"]
            .contains("exports = [\"@rebaze_maven//:org_hamcrest_hamcrest_core\"]")
    );
    assert!(!files["core/BUILD.bazel"].contains("java_test("));
}

#[test]
fn uncertain_java_dependency_choices_fail_before_output_bundle() {
    let mut unsupported = java_project();
    unsupported.modules[1].plugins[0].id = "org.jetbrains.kotlin.jvm".into();
    assert!(
        generate(&unsupported)
            .unwrap_err()
            .to_string()
            .contains("plugins")
    );
    for source in [
        "dependencies { implementation project(':missing') }",
        "dependencies { implementation project('core') }",
        "dependencies { compileOnly 'junit:junit:4.13.2' }",
        "dependencies { testImplementation 'junit:junit:4.+' }",
        "dependencies { testImplementation 'junit:junit' }",
        "dependencies { implementation 'junit:junit:4.12' }",
    ] {
        let mut project = java_project();
        project.modules[1].dependencies.extend(
            rebaze_gradle::parse_build_source(source)
                .unwrap()
                .dependencies,
        );
        assert!(generate(&project).is_err(), "{source}");
    }
    let mut project = java_project();
    project.modules[0]
        .dependencies
        .push(rebaze_gradle::Dependency::Project {
            configuration: "implementation".into(),
            path: ":app".into(),
        });
    assert!(
        generate(&project)
            .unwrap_err()
            .to_string()
            .contains("cyclic")
    );
}

#[test]
fn nested_gradle_includes_retain_implicit_parent_builds() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(temp.path().join("app/core")).unwrap();
    std::fs::write(temp.path().join("settings.gradle"), "include('app:core')").unwrap();
    std::fs::write(
        temp.path().join("app/build.gradle"),
        "plugins { id 'java' }",
    )
    .unwrap();
    std::fs::write(
        temp.path().join("app/core/build.gradle"),
        "plugins { id 'java-library' }",
    )
    .unwrap();
    let project = rebaze_gradle::parse(temp.path()).unwrap();
    assert_eq!(
        project
            .modules
            .iter()
            .map(|m| m.path.as_str())
            .collect::<Vec<_>>(),
        vec!["", ":app", ":app:core"]
    );
    let files = generate(&project).unwrap();
    assert!(files["app/BUILD.bazel"].contains("java_library("));
    assert!(files["app/core/BUILD.bazel"].contains("java_library("));
}

#[test]
fn gradle_maven_lock_must_be_a_regular_file() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(
        temp.path().join("settings.gradle"),
        "rootProject.name = 'example'",
    )
    .unwrap();
    std::fs::write(temp.path().join("build.gradle"), "plugins { id 'java' }").unwrap();
    std::fs::create_dir(temp.path().join("maven_install.json")).unwrap();
    assert!(
        rebaze_gradle::parse(temp.path())
            .unwrap_err()
            .to_string()
            .contains("regular project file")
    );
}

#[cfg(unix)]
#[test]
fn gradle_source_scan_rejects_symlinked_ancestor_directories() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("project");
    let outside = temp.path().join("outside");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(outside.join("main/java")).unwrap();
    std::fs::write(root.join("settings.gradle"), "rootProject.name = 'example'").unwrap();
    std::fs::write(root.join("build.gradle"), "plugins { id 'java' }").unwrap();
    std::fs::write(
        outside.join("main/java/Outside.java"),
        "public class Outside {}",
    )
    .unwrap();
    std::os::unix::fs::symlink(&outside, root.join("src")).unwrap();
    assert!(
        rebaze_gradle::parse(&root)
            .unwrap_err()
            .to_string()
            .contains("source symlinks")
    );
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
    assert!(crate::plan_cmake(&project, &MigrationConfig::default()).is_err());
    project.pkg_config_modules[0].prefix = "safe_alias".into();
    project.pkg_config_modules[0].packages = vec!["safe-package".into()];
    let files = generate_from_cmake_with_config(
        &project,
        &MigrationConfig::default(),
        &unavailable(&project),
    )
    .unwrap();
    assert_syntax(&files);
    assert_eq!(files.len(), 8);
    assert!(!files["third_party/BUILD.bazel"].contains("@rules_foreign_cc"));
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
    let mut config = MigrationConfig::default();
    config.strategy.default = "source".into();
    let files =
        generate_from_cmake_with_config(&project, &config, &DiscoverySnapshot::default()).unwrap();
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
    config.strategy.default = "source".into();
    let glib = config.mappings.known_packages.get_mut("glib").unwrap();
    glib.url = "https://example.invalid/custom-glib.tar.xz".into();
    glib.build_options = vec!["-Dtests=false".into(), "-Dcustom_flag".into()];
    glib.deps = vec!["zlib".into()];
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
    assert!(files["MODULE.bazel"].contains("glib_src"));
    assert!(files["MODULE.bazel"].contains("custom_dep_src"));
    assert!(build.contains("//third_party:custom_dep_source"));
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
    assert!(expected["third_party/system_deps.bzl"].contains("-lcustom"));
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
    assert!(files["third_party/BUILD.bazel"].contains("@system_my_lib//:my_lib"));
    assert!(!files["third_party/BUILD.bazel"].contains("-lmy"));
    assert!(
        !files["third_party/system_deps.bzl"]
            .split("def _bazel_option")
            .next()
            .unwrap()
            .contains("\"path\":")
    );
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
            modules: vec![java_module("")],
            maven_lock: None,
        })
        .unwrap();
    });
}

#[test]
fn native_target_collisions_fail_before_discovery() {
    for declarations in [
        "add_library(foo-bar a.c)\nadd_library(foo_bar b.c)",
        "add_library(same a.c)\nadd_executable(same b.c)",
        "add_executable(app-x a.c)\nadd_executable(app_x b.c)",
    ] {
        let file =
            rebaze_cmake::parse_source(&format!("project(example C)\n{declarations}")).unwrap();
        let project = rebaze_cmake::extract_project(&file, ".".into());
        assert!(
            crate::plan_cmake(&project, &MigrationConfig::default()).is_err(),
            "accepted colliding targets: {declarations}"
        );
    }
}

#[test]
fn imported_names_must_not_match_an_unrelated_local_component() {
    let file = rebaze_cmake::parse_source("project(example C)\nadd_library(core core.c)\nadd_executable(app main.c)\ntarget_link_libraries(app Unknown::core)").unwrap();
    let project = rebaze_cmake::extract_project(&file, ".".into());
    assert!(crate::plan_cmake(&project, &MigrationConfig::default()).is_err());
}

#[test]
fn managed_aliases_are_attached_only_to_their_consumers() {
    let file = rebaze_cmake::parse_source("project(example C)\npkg_check_modules(A REQUIRED IMPORTED_TARGET zlib)\npkg_check_modules(B REQUIRED IMPORTED_TARGET expat)\nadd_executable(first first.c)\ntarget_link_libraries(first PkgConfig::A)\nadd_executable(second second.c)\ntarget_link_libraries(second PkgConfig::B)").unwrap();
    let project = rebaze_cmake::extract_project(&file, ".".into());
    let files = generate_from_cmake_with_config(
        &project,
        &MigrationConfig::default(),
        &unavailable(&project),
    )
    .unwrap();
    let build = &files["BUILD.bazel"];
    let first = build
        .split("cc_binary(")
        .nth(1)
        .unwrap()
        .split("cc_binary(")
        .next()
        .unwrap();
    let second = build.split("cc_binary(").nth(2).unwrap();
    assert!(first.contains("\"//third_party:a\""));
    assert!(!first.contains("\"//third_party:b\""));
    assert!(second.contains("\"//third_party:b\""));
    assert!(!second.contains("\"//third_party:a\""));
}

#[test]
fn native_alias_graph_is_validated_even_without_consumers() {
    for aliases in [
        vec![("Alias::one", "Alias::two"), ("Alias::two", "Alias::one")],
        vec![("Alias::missing", "unknown_required")],
        vec![("core", "other")],
        vec![("Bad::", "core")],
    ] {
        let file =
            rebaze_cmake::parse_source("project(example C)\nadd_library(core core.c)").unwrap();
        let mut project = rebaze_cmake::extract_project(&file, ".".into());
        project
            .aliases
            .extend(aliases.into_iter().map(|(a, b)| (a.into(), b.into())));
        assert!(crate::plan_cmake(&project, &MigrationConfig::default()).is_err());
    }
}

#[test]
fn native_alias_chains_resolve_exact_local_and_configured_targets() {
    let file = rebaze_cmake::parse_source("project(example C)\nadd_library(core core.c)\nadd_executable(app main.c)\ntarget_link_libraries(app Alias::a0 Alias::ssl)").unwrap();
    let mut project = rebaze_cmake::extract_project(&file, ".".into());
    for index in 0..4096 {
        project.aliases.insert(
            format!("Alias::a{index}"),
            if index == 4095 {
                "core".into()
            } else {
                format!("Alias::a{}", index + 1)
            },
        );
    }
    project
        .aliases
        .insert("Alias::ssl".into(), "OpenSSL::SSL".into());
    let files = generate_from_cmake_with_config(
        &project,
        &MigrationConfig::default(),
        &DiscoverySnapshot::default(),
    )
    .unwrap();
    assert_syntax(&files);
    assert_eq!(files["BUILD.bazel"].matches("\":core\"").count(), 1);
    assert_eq!(
        files["BUILD.bazel"].matches("\"@openssl//:ssl\"").count(),
        1
    );
}

#[test]
fn invalid_native_names_and_undeclared_pkg_aliases_fail_preparation() {
    for name in ["", ".", "..", "bad/name", "name:part", "nonasciié"] {
        let mut project = rebaze_cmake::CMakeProject::default();
        project
            .executables
            .push(rebaze_cmake::Executable::new(name.into()));
        assert!(
            crate::plan_cmake(&project, &MigrationConfig::default()).is_err(),
            "accepted {name:?}"
        );
    }
    for dependency in ["PkgConfig::undeclared", "other"] {
        let file = rebaze_cmake::parse_source(&format!("project(example C)\nadd_executable(app main.c)\nadd_executable(other other.c)\ntarget_link_libraries(app {dependency})")).unwrap();
        let project = rebaze_cmake::extract_project(&file, ".".into());
        assert!(crate::plan_cmake(&project, &MigrationConfig::default()).is_err());
    }
}
