#![allow(clippy::unwrap_used)]

use crate::discovery::{ProbeOutcome, UnavailableReason};
use crate::{DiscoverySnapshot, MigrationConfig, plan_cmake};
use rebaze_cmake::{CMakeProject, PkgConfigModule};

fn project(prefix: &str, packages: &[&str]) -> CMakeProject {
    CMakeProject {
        name: "example".into(),
        pkg_config_modules: vec![PkgConfigModule {
            prefix: prefix.into(),
            packages: packages.iter().map(|p| (*p).into()).collect(),
            required: true,
        }],
        ..Default::default()
    }
}

fn render(
    project: &CMakeProject,
    config: &MigrationConfig,
) -> crate::GenerationResult<std::collections::HashMap<String, String>> {
    let prepared = plan_cmake(project, config)?;
    let mut snapshot = DiscoverySnapshot::default();
    for module in prepared.system_modules() {
        snapshot.record(
            module,
            ProbeOutcome::Unavailable(UnavailableReason::ToolNotFound),
        )?;
    }
    prepared.render(&snapshot)
}

#[test]
fn default_source_uses_package_identity_and_activates_only_source_repositories() {
    let project = project("COMPRESSION", &["zlib>=1.2"]);
    let mut config = MigrationConfig::default();
    config.strategy.default = "source".into();
    let prepared = plan_cmake(&project, &config).unwrap();
    assert!(prepared.system_modules().is_empty());
    let files = prepared.render(&DiscoverySnapshot::default()).unwrap();
    assert!(files["MODULE.bazel"].contains("\nsource_deps = use_extension("));
    assert!(files["MODULE.bazel"].contains("\"zlib_src\""));
    assert!(!files["MODULE.bazel"].contains("system_deps"));
    assert!(files["third_party/BUILD.bazel"].contains("actual = \"//third_party:zlib_source\""));
    assert!(files["third_party/config.bzl"].contains("\"source\""));
    assert!(!files["third_party/source.bzl"].contains("glib-2.82"));
}

#[test]
fn prefix_override_wins_over_package_override_and_default() {
    let project = project("COMPRESSION", &["zlib"]);
    let mut config = MigrationConfig::default();
    config.strategy.default = "source".into();
    config
        .strategy
        .overrides
        .insert("zlib".into(), "bcr".into());
    config
        .strategy
        .overrides
        .insert("compression".into(), "system".into());
    let prepared = plan_cmake(&project, &config).unwrap();
    assert_eq!(prepared.system_modules().len(), 1);
    assert_eq!(
        prepared.system_modules()[0].packages,
        project.pkg_config_modules[0].packages
    );
    let files = render(&project, &config).unwrap();
    assert!(!files["MODULE.bazel"].contains("rules_foreign_cc"));
    assert!(!files["MODULE.bazel"].contains("source_deps"));
    assert!(!files["third_party/BUILD.bazel"].contains("_source\""));
    config.strategy.overrides.remove("compression");
    assert!(
        plan_cmake(&project, &config)
            .unwrap()
            .system_modules()
            .is_empty()
    );
    assert!(
        render(&project, &config).unwrap()["third_party/BUILD.bazel"]
            .contains("actual = \"@zlib\"")
    );
}

#[test]
fn bcr_keeps_all_targets_when_module_imports_are_deduplicated() {
    let project = project("TEST_FRAMEWORKS", &["gtest", "gmock"]);
    let mut config = MigrationConfig::default();
    config.strategy.default = "bcr".into();
    config
        .strategy
        .bcr
        .module_overrides
        .insert("googletest".into(), "replacement".into());
    config
        .strategy
        .bcr
        .version_pins
        .insert("replacement".into(), "1.2.3".into());
    let files = render(&project, &config).unwrap();
    assert_eq!(
        files["MODULE.bazel"]
            .matches("name = \"replacement\"")
            .count(),
        1
    );
    assert!(files["MODULE.bazel"].contains("version = \"1.2.3\""));
    let build = &files["third_party/BUILD.bazel"];
    assert!(build.contains("@replacement//:gtest"));
    assert!(build.contains("@replacement//:gmock"));
    assert!(!build.contains("@googletest"));
    assert!(!files["MODULE.bazel"].contains("use_extension"));
}

#[test]
fn bcr_skip_and_fallback_select_real_system_or_source_edges() {
    let project = project("Z", &["zlib"]);
    let mut config = MigrationConfig::default();
    config.strategy.default = "bcr".into();
    config.strategy.bcr.skip_packages.push("zlib".into());
    assert_eq!(
        plan_cmake(&project, &config).unwrap().system_modules()[0].packages,
        ["zlib"]
    );
    config.strategy.bcr_fallback = Some("source".into());
    let files = render(&project, &config).unwrap();
    assert!(files["third_party/BUILD.bazel"].contains("//third_party:zlib_source"));
    assert!(!files["MODULE.bazel"].contains("system_deps"));
}

#[test]
fn source_transitives_can_use_system_requests_with_catalog_pkg_names() {
    let project = project("GLIB", &["glib-2.0"]);
    let mut config = MigrationConfig::default();
    config.strategy.default = "source".into();
    config
        .strategy
        .overrides
        .insert("pcre2".into(), "system".into());
    let prepared = plan_cmake(&project, &config).unwrap();
    assert_eq!(prepared.system_modules().len(), 1);
    assert_eq!(prepared.system_modules()[0].packages, ["libpcre2-8"]);
    let files = render(&project, &config).unwrap();
    assert!(files["third_party/BUILD.bazel"].contains("@system_pcre2//:pcre2"));
    assert!(!files["third_party/BUILD.bazel"].contains("name = \"pcre2_source\""));
    assert!(!files["MODULE.bazel"].contains("pcre2_src"));
}

#[test]
fn parsed_pkg_config_links_use_managed_aliases_for_binaries_and_libraries() {
    let parsed = rebaze_cmake::parse_source("project(example)\npkg_check_modules(Z REQUIRED zlib)\nadd_library(core STATIC core.c)\ntarget_link_libraries(core PRIVATE ${Z_LIBRARIES})\nadd_executable(app main.c)\ntarget_link_libraries(app PRIVATE core ${Z_LIBRARIES} PkgConfig::Z)").unwrap();
    let project = rebaze_cmake::extract_project(&parsed, ".".into());
    assert!(
        project.libraries[0]
            .link_libraries
            .contains(&"-lzlib".into())
    );
    for strategy in ["system", "source", "bcr"] {
        let mut config = MigrationConfig::default();
        config.strategy.default = strategy.into();
        let files = render(&project, &config).unwrap();
        assert_eq!(files["BUILD.bazel"].matches("//third_party:z\"").count(), 2);
        assert!(!files["BUILD.bazel"].contains("@zlib"), "{strategy}");
    }
}

#[test]
fn invalid_strategy_spellings_are_rejected_even_when_inactive() {
    let project = CMakeProject::default();
    for strategy in ["conan", "vcpkg", "prebuilt", "bazelle", "custom", "typo"] {
        let mut config = MigrationConfig::default();
        config.strategy.default = strategy.into();
        assert!(
            plan_cmake(&project, &config)
                .unwrap_err()
                .to_string()
                .contains("unsupported dependency strategy")
        );
        config.strategy.default = "system".into();
        config
            .strategy
            .overrides
            .insert("unused".into(), strategy.into());
        assert!(plan_cmake(&project, &config).is_err());
    }
    let mut config = MigrationConfig::default();
    config.strategy.bcr_fallback = Some("bcr".into());
    assert!(plan_cmake(&project, &config).is_err());
}

#[test]
fn source_metadata_is_required_only_for_active_sources_and_cycles_fail() {
    let input = project("ZLIB", &["zlib"]);
    let mut config = MigrationConfig::default();
    config
        .mappings
        .known_packages
        .get_mut("zlib")
        .unwrap()
        .sha256
        .clear();
    assert!(render(&input, &config).is_ok());
    config.strategy.default = "source".into();
    assert!(
        plan_cmake(&input, &config)
            .unwrap_err()
            .to_string()
            .contains("SHA256")
    );
    let mut config = MigrationConfig::default();
    config.strategy.default = "source".into();
    config.mappings.known_packages.get_mut("zlib").unwrap().deps = vec!["zlib".into()];
    assert!(
        plan_cmake(&input, &config)
            .unwrap_err()
            .to_string()
            .contains("cycle")
    );
    assert!(plan_cmake(&project("UNKNOWN", &["unknown"]), &config).is_err());
}

#[test]
fn normalized_alias_source_target_and_request_collisions_fail() {
    let mut project = project("MY-LIB", &["zlib"]);
    project.pkg_config_modules.push(PkgConfigModule {
        prefix: "my_lib".into(),
        packages: vec!["libffi".into()],
        required: true,
    });
    assert!(
        plan_cmake(&project, &MigrationConfig::default())
            .unwrap_err()
            .to_string()
            .contains("conflicting requests")
    );
    project.pkg_config_modules[1].packages = vec!["zlib".into()];
    assert!(plan_cmake(&project, &MigrationConfig::default()).is_ok());
    project.pkg_config_modules[1].prefix = "zlib_source".into();
    project.pkg_config_modules[1].packages = vec!["libffi".into()];
    let mut config = MigrationConfig::default();
    config.strategy.default = "source".into();
    assert!(
        plan_cmake(&project, &config)
            .unwrap_err()
            .to_string()
            .contains("collides")
    );
}

#[test]
fn conflicting_package_overrides_require_an_explicit_prefix_choice() {
    let project = project("ALL", &["zlib", "libffi"]);
    let mut config = MigrationConfig::default();
    config
        .strategy
        .overrides
        .insert("zlib".into(), "source".into());
    config
        .strategy
        .overrides
        .insert("libffi".into(), "system".into());
    assert!(plan_cmake(&project, &config).is_err());
    config
        .strategy
        .overrides
        .insert("all".into(), "source".into());
    assert!(plan_cmake(&project, &config).is_ok());
}

#[test]
fn source_and_bcr_version_predicates_are_checked() {
    for strategy in ["source", "bcr"] {
        let mut config = MigrationConfig::default();
        config.strategy.default = strategy.into();
        assert!(
            plan_cmake(&project("Z", &["zlib>=999.0"]), &config)
                .unwrap_err()
                .to_string()
                .contains("does not satisfy")
        );
        assert!(plan_cmake(&project("Z", &["zlib>=1.2"]), &config).is_ok());
        assert!(plan_cmake(&project("Z", &["zlib>=1.x"]), &config).is_err());
        assert!(plan_cmake(&project("Z", &[" zlib>=999.0 "]), &config).is_err());
    }
}

#[test]
fn every_source_build_system_receives_its_options() {
    use crate::config::SourceBuildSystem;
    let project = project("Z", &["zlib"]);
    for (system, option, field) in [
        (
            SourceBuildSystem::Cmake,
            "-DBUILD_TESTING=OFF",
            "generate_args",
        ),
        (
            SourceBuildSystem::Autotools,
            "--disable-shared",
            "configure_options",
        ),
        (SourceBuildSystem::Meson, "-Dtests=false", "options"),
    ] {
        let mut config = MigrationConfig::default();
        config.strategy.default = "source".into();
        let info = config.mappings.known_packages.get_mut("zlib").unwrap();
        info.build_system = system;
        info.build_options = vec![option.into()];
        let build = &render(&project, &config).unwrap()["third_party/BUILD.bazel"];
        assert!(build.contains(field));
        if system == SourceBuildSystem::Meson {
            assert!(build.contains("\"tests\": \"false\""));
        } else {
            assert!(build.contains(option));
        }
    }
}

#[test]
fn system_transitive_rebinding_preserves_version_constraints() {
    let project = project("GLIB", &["glib-2.0"]);
    let mut config = MigrationConfig::default();
    config.strategy.default = "source".into();
    config.mappings.known_packages.get_mut("glib").unwrap().deps = vec!["pcre2>=999.0".into()];
    config
        .strategy
        .overrides
        .insert("pcre2".into(), "system".into());
    let prepared = plan_cmake(&project, &config).unwrap();
    assert_eq!(prepared.system_modules()[0].packages, ["libpcre2-8>=999.0"]);
    config
        .mappings
        .known_packages
        .get_mut("pcre2")
        .unwrap()
        .pkg_config_packages
        .push("another-pcre".into());
    assert!(
        plan_cmake(&project, &config)
            .unwrap_err()
            .to_string()
            .contains("cannot rebind")
    );
}

#[test]
fn ambiguous_selected_source_identity_fails_without_affecting_system_defaults() {
    let project = project("DEP", &["ambiguous"]);
    let mut config = MigrationConfig::default();
    for name in ["candidate_a", "candidate_b"] {
        let mut info = config.mappings.known_packages["zlib"].clone();
        info.pkg_config_packages = vec!["ambiguous".into()];
        config.mappings.known_packages.insert(name.into(), info);
    }
    assert!(plan_cmake(&project, &config).is_ok());
    config.strategy.default = "source".into();
    assert!(
        plan_cmake(&project, &config)
            .unwrap_err()
            .to_string()
            .contains("ambiguous source identity")
    );
}

#[test]
fn dotted_version_component_count_matches_pkg_config_ordering() {
    let mut config = MigrationConfig::default();
    config.strategy.default = "source".into();
    config
        .mappings
        .known_packages
        .get_mut("zlib")
        .unwrap()
        .version = "1.2".into();
    for predicate in ["=1.2.0", ">=1.2.0"] {
        assert!(plan_cmake(&project("Z", &[&format!("zlib{predicate}")]), &config).is_err());
    }
    assert!(plan_cmake(&project("Z", &["zlib<1.2.0"]), &config).is_ok());
    assert!(plan_cmake(&project("Z", &["zlib=1.2"]), &config).is_ok());
}

#[test]
fn apparent_repository_names_and_bazel_module_names_are_validated_together() {
    let mut project = project("Z", &["zlib"]);
    project.pkg_config_modules.push(PkgConfigModule {
        prefix: "OTHER".into(),
        packages: vec!["fmt".into()],
        required: true,
    });
    let mut config = MigrationConfig::default();
    config.strategy.default = "source".into();
    config
        .strategy
        .overrides
        .insert("other".into(), "bcr".into());
    config
        .strategy
        .bcr
        .module_overrides
        .insert("fmt".into(), "zlib_src".into());
    assert!(
        plan_cmake(&project, &config)
            .unwrap_err()
            .to_string()
            .contains("apparent repository")
    );
    config
        .strategy
        .overrides
        .insert("z".into(), "system".into());
    config
        .strategy
        .bcr
        .module_overrides
        .insert("fmt".into(), "system_z".into());
    assert!(
        plan_cmake(&project, &config)
            .unwrap_err()
            .to_string()
            .contains("apparent repository")
    );
    for invalid in [
        "Uppercase",
        "plus+module",
        "7module",
        "replacement_",
        "replacement-",
        "replacement.",
    ] {
        config
            .strategy
            .bcr
            .module_overrides
            .insert("fmt".into(), invalid.into());
        assert!(
            plan_cmake(&project, &config)
                .unwrap_err()
                .to_string()
                .contains("invalid Bazel module name")
        );
    }
}

#[test]
fn repeated_normalized_alias_registers_all_imported_target_spellings() {
    for prefixes in [["z", "Z"], ["Z", "z"]] {
        let parsed = rebaze_cmake::parse_source(&format!("project(example)\npkg_check_modules({} zlib)\npkg_check_modules({} zlib)\nadd_executable(app main.c)\ntarget_link_libraries(app PRIVATE PkgConfig::Z PkgConfig::z)", prefixes[0], prefixes[1])).unwrap();
        let project = rebaze_cmake::extract_project(&parsed, ".".into());
        let files = render(&project, &MigrationConfig::default()).unwrap();
        assert!(!files["BUILD.bazel"].contains("//third_party:Z"));
        assert_eq!(files["BUILD.bazel"].matches("//third_party:z\"").count(), 1);
    }
}

#[test]
fn serialized_plan_is_data_and_contains_no_undefined_constructors() {
    let mut config = MigrationConfig::default();
    config.strategy.default = "source".into();
    let files = render(&project("Z", &["zlib"]), &config).unwrap();
    for name in ["PlannedAlias(", "SourceArchive(", "SystemConfiguration("] {
        assert!(
            files.values().all(|source| !source.contains(name)),
            "{name}"
        );
    }
    assert!(
        files["third_party/config.bzl"].contains("\"targets\": [\"//third_party:zlib_source\"]")
    );
}

#[test]
fn shorthand_bcr_targets_keep_their_target_name_after_a_module_override() {
    let mut config = MigrationConfig::default();
    config.strategy.default = "bcr".into();
    config
        .strategy
        .bcr
        .module_overrides
        .insert("zlib".into(), "replacement".into());
    let files = render(&project("Z", &["zlib"]), &config).unwrap();
    assert!(files["third_party/BUILD.bazel"].contains("@replacement//:zlib"));
}

#[test]
fn static_meson_catalog_outputs_request_static_builds() {
    let mut config = MigrationConfig::default();
    config.strategy.default = "source".into();
    let files = render(&project("GOBJECT", &["gobject-2.0"]), &config).unwrap();
    assert_eq!(
        files["third_party/BUILD.bazel"]
            .matches("\"default_library\": \"static\"")
            .count(),
        2
    );
}

#[test]
fn default_source_generation_does_not_require_host_observations() {
    // Uses the old public entry point too, so this regression can run on the base.
    let project = rebaze_cmake::CMakeProject {
        name: "regression".into(),
        pkg_config_modules: vec![rebaze_cmake::PkgConfigModule {
            prefix: "COMPRESSION".into(),
            packages: vec!["zlib".into()],
            required: true,
        }],
        ..Default::default()
    };
    let mut config = MigrationConfig::default();
    config.strategy.default = "source".into();
    let files =
        crate::generate_from_cmake_with_config(&project, &config, &DiscoverySnapshot::default())
            .unwrap();
    assert!(files["MODULE.bazel"].contains("\nsource_deps = use_extension("));
    assert!(!files["MODULE.bazel"].contains("system_deps"));
}

#[test]
fn uncatalogued_system_transitives_keep_their_original_predicates() {
    let mut config = MigrationConfig::default();
    config.strategy.default = "source".into();
    config.mappings.known_packages.get_mut("zlib").unwrap().deps = vec!["uncatalogued>=1.2".into()];
    config
        .strategy
        .overrides
        .insert("uncatalogued".into(), "system".into());
    let input = project("Z", &["zlib"]);
    let prepared = plan_cmake(&input, &config).unwrap();
    assert_eq!(prepared.system_modules()[0].packages, ["uncatalogued>=1.2"]);
}

#[test]
fn planning_limits_accept_the_boundary_and_reject_the_next_item() {
    let mut input = CMakeProject::default();
    let config = MigrationConfig::default();
    for index in 0..1024 {
        input.pkg_config_modules.push(PkgConfigModule {
            prefix: format!("p{index}"),
            packages: vec![format!("p{index}")],
            required: true,
        });
    }
    assert_eq!(
        plan_cmake(&input, &config).unwrap().system_modules().len(),
        1024
    );
    input.pkg_config_modules.push(PkgConfigModule {
        prefix: "overflow".into(),
        packages: vec!["overflow".into()],
        required: true,
    });
    assert!(
        plan_cmake(&input, &config)
            .unwrap_err()
            .to_string()
            .contains("1024 aliases")
    );
    let mut input = project("ALL", &[]);
    input.pkg_config_modules[0].packages = (0..256).map(|index| format!("p{index}")).collect();
    assert!(plan_cmake(&input, &config).is_ok());
    input.pkg_config_modules[0].packages.push("overflow".into());
    assert!(plan_cmake(&input, &config).is_err());

    let mut config = MigrationConfig::default();
    config.strategy.default = "source".into();
    for index in 0..64 {
        let mut info = config.mappings.known_packages["zlib"].clone();
        info.deps = if index == 63 {
            vec![]
        } else {
            vec![format!("chain_{}", index + 1)]
        };
        config
            .mappings
            .known_packages
            .insert(format!("chain_{index}"), info);
    }
    let input = project("CHAIN", &["chain_0"]);
    assert!(plan_cmake(&input, &config).is_ok());
    config
        .mappings
        .known_packages
        .get_mut("chain_63")
        .unwrap()
        .deps = vec!["zlib".into()];
    assert!(
        plan_cmake(&input, &config)
            .unwrap_err()
            .to_string()
            .contains("64 levels")
    );
}

#[test]
fn invalid_active_source_paths_outputs_and_options_fail_planning() {
    let input = project("Z", &["zlib"]);
    for (strip_prefix, output) in [
        ("../escape", "libz.a"),
        ("zlib", "/absolute.a"),
        ("zlib", "../escape.a"),
        ("zlib", ""),
    ] {
        let mut config = MigrationConfig::default();
        config.strategy.default = "source".into();
        let info = config.mappings.known_packages.get_mut("zlib").unwrap();
        info.strip_prefix = strip_prefix.into();
        info.out_static_libs = vec![output.into()];
        assert!(plan_cmake(&input, &config).is_err());
    }
    let mut config = MigrationConfig::default();
    config.strategy.default = "source".into();
    let info = config.mappings.known_packages.get_mut("zlib").unwrap();
    info.build_system = crate::config::SourceBuildSystem::Meson;
    info.build_options = vec!["-Dtests=true".into(), "tests=false".into()];
    assert!(
        plan_cmake(&input, &config)
            .unwrap_err()
            .to_string()
            .contains("duplicate Meson option")
    );
}

#[test]
fn checked_in_configuration_example_preserves_catalog_defaults_and_can_select_source() {
    let content = include_str!("../../../rebaze.example.toml");
    let mut config: MigrationConfig = toml::from_str(content).unwrap();
    assert!(config.mappings.known_packages.contains_key("zlib"));
    config
        .strategy
        .overrides
        .insert("zlib".into(), "source".into());
    let files = render(&project("COMPRESSION", &["zlib"]), &config).unwrap();
    assert!(files["third_party/BUILD.bazel"].contains("//third_party:zlib_source"));
}

#[test]
fn partial_mapping_tables_keep_omitted_defaults_and_merge_source_entries() {
    let mut config: MigrationConfig =
        toml::from_str("[mappings]\nignored_libraries = []\n").unwrap();
    assert!(config.mappings.known_packages.contains_key("zlib"));
    assert!(config.mappings.transitive_deps.contains_key("glib"));
    assert!(config.mappings.plain_libraries.contains_key("gtest"));
    assert_eq!(config.mappings.ignored_libraries, Vec::<String>::new());
    let mut custom = config.mappings.known_packages["zlib"].clone();
    custom.version = "9.9".into();
    config.mappings.known_packages.clear();
    config
        .mappings
        .known_packages
        .insert("custom".into(), custom);
    let roundtrip: MigrationConfig = toml::from_str(&toml::to_string(&config).unwrap()).unwrap();
    assert!(roundtrip.mappings.known_packages.contains_key("zlib"));
    assert_eq!(roundtrip.mappings.known_packages["custom"].version, "9.9");
}
