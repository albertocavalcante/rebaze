//! Explicit compiler oracle for configured defaults; ordinary tests stay offline.
#![cfg(unix)]
#![allow(clippy::unwrap_used)]

#[test]
#[ignore = "explicit oracle: Bazel 9, C/C++ toolchains and network"]
fn mixed_language_defaults_compile_in_the_correct_language() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join("value.c"),
        "#if !defined(__STDC_VERSION__) || __STDC_VERSION__ < 201710L\n#error C17 was not selected\n#endif\nint value(void) { return 42; }\n",
    )
    .unwrap();
    std::fs::write(
        root.path().join("main.cpp"),
        "#include <cstdio>\n#if __cplusplus < 202002L\n#error C++20 was not selected\n#endif\nextern \"C\" int value(void);\nint main() { int n = value(); std::printf(\"rebaze dependency fixture: %d\\n\", n); return n != 42; }\n",
    )
    .unwrap();
    let parsed = rebaze_cmake::parse_source(
        "project(mixed_defaults C CXX)\nadd_executable(app main.cpp value.c)",
    )
    .unwrap();
    let project = rebaze_cmake::extract_project(&parsed, root.path().into());
    let mut config = crate::MigrationConfig::default();
    config.build.default_c_standard = "17".into();
    config.build.default_cxx_standard = "20".into();
    config.build.default_visibility = "//visibility:private".into();
    let files = crate::plan_cmake(&project, &config)
        .unwrap()
        .render(&crate::DiscoverySnapshot::default())
        .unwrap();
    crate::dependency_build_tests::build_and_run(root.path(), &files);
}
