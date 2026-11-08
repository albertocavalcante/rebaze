use super::*;
use rebaze_input::{LimitKind, ReadError};
use std::fs;

#[test]
fn recursive_capture_uses_one_file_and_byte_budget() {
    let root = tempfile::tempdir().unwrap();
    let parent = "project(example)\nadd_subdirectory(child)\n";
    let child = "add_library(child child.c)\n";
    fs::create_dir(root.path().join("child")).unwrap();
    fs::write(root.path().join("CMakeLists.txt"), parent).unwrap();
    fs::write(root.path().join("child/CMakeLists.txt"), child).unwrap();
    let total = parent.len() + child.len();
    let limits = ReadLimits::new(parent.len(), total, 2).unwrap();
    let project = extract_project_from_path_with_limits(root.path(), limits).unwrap();
    assert_eq!(project.libraries[0].sources, ["child/child.c"]);
    let limits = ReadLimits::new(parent.len(), total - 1, 2).unwrap();
    assert!(matches!(
        extract_project_from_path_with_limits(root.path(), limits),
        Err(ExtractError::Input(ReadError::Limit {
            kind: LimitKind::AggregateBytes,
            ..
        }))
    ));
    let limits = ReadLimits::new(parent.len(), total, 1).unwrap();
    assert!(matches!(
        extract_project_from_path_with_limits(root.path(), limits),
        Err(ExtractError::Input(ReadError::Limit {
            kind: LimitKind::FileAttempts,
            ..
        }))
    ));
}

#[test]
fn wide_and_deep_projects_stop_at_shared_limits() {
    let root = tempfile::tempdir().unwrap();
    fs::write(
        root.path().join("CMakeLists.txt"),
        "project(example)\nadd_subdirectory(a)\nadd_subdirectory(b)\n",
    )
    .unwrap();
    for name in ["a", "b"] {
        fs::create_dir(root.path().join(name)).unwrap();
        fs::write(root.path().join(name).join("CMakeLists.txt"), "# empty").unwrap();
    }
    assert!(matches!(
        extract_project_from_path_with_limits(root.path(), ReadLimits::new(128, 256, 2).unwrap()),
        Err(ExtractError::Input(ReadError::Limit {
            kind: LimitKind::FileAttempts,
            ..
        }))
    ));
    let limits = ReadLimits::new(128, 256, 3)
        .unwrap()
        .with_traversal(100, 1)
        .unwrap();
    assert!(matches!(
        extract_project_from_path_with_limits(root.path(), limits),
        Err(ExtractError::Input(ReadError::Limit {
            kind: LimitKind::PathDepth,
            ..
        }))
    ));
}

#[cfg(unix)]
#[test]
fn child_links_stay_under_original_root_and_repeated_aliases_are_rejected() {
    use std::os::unix::fs::symlink;
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join("real")).unwrap();
    fs::write(
        root.path().join("real/CMakeLists.txt"),
        "add_library(child child.c)",
    )
    .unwrap();
    symlink("real", root.path().join("alias")).unwrap();
    fs::write(
        root.path().join("CMakeLists.txt"),
        "add_subdirectory(alias)",
    )
    .unwrap();
    assert_eq!(
        extract_project_from_path(root.path())
            .unwrap()
            .libraries
            .len(),
        1
    );
    fs::write(
        root.path().join("CMakeLists.txt"),
        "add_subdirectory(real)\nadd_subdirectory(alias)",
    )
    .unwrap();
    assert!(matches!(
        extract_project_from_path(root.path()),
        Err(ExtractError::RepeatedDirectory(_))
    ));
    fs::remove_file(root.path().join("alias")).unwrap();
    symlink(outside.path(), root.path().join("alias")).unwrap();
    fs::write(
        outside.path().join("CMakeLists.txt"),
        "add_library(outside outside.c)",
    )
    .unwrap();
    fs::write(
        root.path().join("CMakeLists.txt"),
        "add_subdirectory(alias)",
    )
    .unwrap();
    assert!(matches!(
        extract_project_from_path(root.path()),
        Err(ExtractError::Input(_))
    ));
}
