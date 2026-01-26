//! Integration tests for CMake to Bazel migration.

#![allow(clippy::unwrap_used)]

use std::path::Path;

#[test]
fn test_generate_bazel_for_hello_cmake() {
    let fixture_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../rebaze-cmake/../../test-fixtures/cmake-examples/01-basic/A-hello-cmake");

    if !fixture_dir.exists() {
        eprintln!("Skipping test: fixture not found");
        return;
    }

    let project = rebaze_cmake::parse(&fixture_dir).unwrap();
    let files = rebaze_bazel::generate_from_cmake(&project);

    // MODULE.bazel, BUILD.bazel, .bazelversion, .bazelrc
    assert_eq!(files.len(), 4);

    // Check MODULE.bazel
    let module = files
        .iter()
        .find(|(name, _)| *name == "MODULE.bazel")
        .unwrap();
    assert!(module.1.contains("module("));
    assert!(module.1.contains("hello_cmake"));
    assert!(module.1.contains("rules_cc"));

    // Check BUILD.bazel
    let build = files
        .iter()
        .find(|(name, _)| *name == "BUILD.bazel")
        .unwrap();
    assert!(build.1.contains("cc_binary("));
    assert!(build.1.contains("hello_cmake"));
    assert!(build.1.contains("main.cpp"));
}

#[test]
fn test_generate_bazel_for_static_library() {
    let fixture_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../rebaze-cmake/../../test-fixtures/cmake-examples/01-basic/C-static-library");

    if !fixture_dir.exists() {
        eprintln!("Skipping test: fixture not found");
        return;
    }

    let project = rebaze_cmake::parse(&fixture_dir).unwrap();
    let files = rebaze_bazel::generate_from_cmake(&project);

    // Check BUILD.bazel contains library and binary
    let build = files
        .iter()
        .find(|(name, _)| *name == "BUILD.bazel")
        .unwrap();
    assert!(build.1.contains("cc_library("));
    assert!(build.1.contains("cc_binary("));
    assert!(build.1.contains("hello_library"));
    assert!(build.1.contains("hello_binary"));

    // Check that binary depends on library
    assert!(build.1.contains(":hello_library"));
}

#[test]
fn test_generate_bazel_for_shared_library() {
    let fixture_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../rebaze-cmake/../../test-fixtures/cmake-examples/01-basic/D-shared-library");

    if !fixture_dir.exists() {
        eprintln!("Skipping test: fixture not found");
        return;
    }

    let project = rebaze_cmake::parse(&fixture_dir).unwrap();
    let files = rebaze_bazel::generate_from_cmake(&project);

    let build = files
        .iter()
        .find(|(name, _)| *name == "BUILD.bazel")
        .unwrap();
    assert!(build.1.contains("cc_library("));
    assert!(build.1.contains("linkstatic = False"));
}

#[test]
fn test_generate_bazel_with_boost() {
    let fixture_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../rebaze-cmake/../../test-fixtures/cmake-examples/01-basic/H-third-party-library");

    if !fixture_dir.exists() {
        eprintln!("Skipping test: fixture not found");
        return;
    }

    let project = rebaze_cmake::parse(&fixture_dir).unwrap();
    let files = rebaze_bazel::generate_from_cmake(&project);

    // Check MODULE.bazel has platform dep (because there are packages)
    let module = files
        .iter()
        .find(|(name, _)| *name == "MODULE.bazel")
        .unwrap();
    assert!(module.1.contains("platforms"));

    // Check BUILD.bazel has dependency on boost
    let build = files
        .iter()
        .find(|(name, _)| *name == "BUILD.bazel")
        .unwrap();
    assert!(build.1.contains("@boost//:filesystem"));
}
