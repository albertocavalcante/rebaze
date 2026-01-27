//! Integration tests using real CMake projects.

#![allow(clippy::unwrap_used)]

use std::path::Path;

// These tests use the ttroy50/cmake-examples repository.
// Clone it to test-fixtures/cmake-examples first.

#[test]
fn test_hello_cmake() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../test-fixtures/cmake-examples/01-basic/A-hello-cmake/CMakeLists.txt");

    if !fixture.exists() {
        eprintln!("Skipping test: fixture not found at {}", fixture.display());
        return;
    }

    let src = std::fs::read_to_string(&fixture).unwrap();
    let file = rebaze_cmake::parse_source(&src).unwrap();

    // Should have: cmake_minimum_required, project, add_executable
    assert_eq!(file.commands.len(), 3);
    assert!(file.commands[0].is("cmake_minimum_required"));
    assert!(file.commands[1].is("project"));
    assert!(file.commands[2].is("add_executable"));

    // Verify project name
    assert_eq!(file.commands[1].arg_literal(0), Some("hello_cmake"));

    // Verify executable
    assert_eq!(file.commands[2].arg_literal(0), Some("hello_cmake"));
    assert_eq!(file.commands[2].arg_literal(1), Some("main.cpp"));
}

#[test]
fn test_hello_headers() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../test-fixtures/cmake-examples/01-basic/B-hello-headers/CMakeLists.txt");

    if !fixture.exists() {
        eprintln!("Skipping test: fixture not found at {}", fixture.display());
        return;
    }

    let src = std::fs::read_to_string(&fixture).unwrap();
    let file = rebaze_cmake::parse_source(&src).unwrap();

    // Should have: cmake_minimum_required, project, set, add_executable, target_include_directories
    assert_eq!(file.commands.len(), 5);
    assert!(file.commands[0].is("cmake_minimum_required"));
    assert!(file.commands[1].is("project"));
    assert!(file.commands[2].is("set"));
    assert!(file.commands[3].is("add_executable"));
    assert!(file.commands[4].is("target_include_directories"));

    // The set command defines SOURCES variable
    assert_eq!(file.commands[2].arg_literal(0), Some("SOURCES"));

    // add_executable uses ${SOURCES} variable - check it's parsed as variable ref
    let exe_args = &file.commands[3].arguments;
    assert_eq!(exe_args.len(), 2); // hello_headers, ${SOURCES}
}

#[test]
fn test_static_library() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../test-fixtures/cmake-examples/01-basic/C-static-library/CMakeLists.txt");

    if !fixture.exists() {
        eprintln!("Skipping test: fixture not found at {}", fixture.display());
        return;
    }

    let src = std::fs::read_to_string(&fixture).unwrap();
    let file = rebaze_cmake::parse_source(&src).unwrap();

    // Should parse add_library STATIC
    let add_lib = file.commands.iter().find(|c| c.is("add_library")).unwrap();
    let args = add_lib.args_literals();
    assert_eq!(args[0], "hello_library");
    assert_eq!(args[1], "STATIC");
}

#[test]
fn test_shared_library() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../test-fixtures/cmake-examples/01-basic/D-shared-library/CMakeLists.txt");

    if !fixture.exists() {
        eprintln!("Skipping test: fixture not found at {}", fixture.display());
        return;
    }

    let src = std::fs::read_to_string(&fixture).unwrap();
    let file = rebaze_cmake::parse_source(&src).unwrap();

    // Should parse add_library SHARED
    let add_lib = file
        .commands
        .iter()
        .find(|c| c.is("add_library") && c.arg_literal(1) == Some("SHARED"))
        .unwrap();
    assert_eq!(add_lib.arg_literal(0), Some("hello_library"));

    // Should also parse ALIAS library
    let alias_lib = file
        .commands
        .iter()
        .find(|c| c.is("add_library") && c.arg_literal(1) == Some("ALIAS"))
        .unwrap();
    assert_eq!(alias_lib.arg_literal(0), Some("hello::library"));
}

#[test]
fn test_project_extraction_hello_cmake() {
    let fixture_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../test-fixtures/cmake-examples/01-basic/A-hello-cmake");

    if !fixture_dir.exists() {
        eprintln!("Skipping test: fixture not found");
        return;
    }

    let project = rebaze_cmake::parse(&fixture_dir).unwrap();

    assert_eq!(project.name, "hello_cmake");
    assert_eq!(project.cmake_minimum_version, Some("3.5".to_string()));
    assert_eq!(project.executables.len(), 1);
    assert_eq!(project.executables[0].name, "hello_cmake");
    assert_eq!(project.executables[0].sources, vec!["main.cpp"]);
}

#[test]
fn test_project_extraction_static_library() {
    let fixture_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../test-fixtures/cmake-examples/01-basic/C-static-library");

    if !fixture_dir.exists() {
        eprintln!("Skipping test: fixture not found");
        return;
    }

    let project = rebaze_cmake::parse(&fixture_dir).unwrap();

    assert_eq!(project.name, "hello_library");
    assert_eq!(project.libraries.len(), 1);
    assert_eq!(project.libraries[0].name, "hello_library");
    assert_eq!(project.libraries[0].kind, rebaze_cmake::LibraryKind::Static);

    assert_eq!(project.executables.len(), 1);
    assert_eq!(project.executables[0].name, "hello_binary");
    // Check link libraries were extracted
    assert!(
        project.executables[0]
            .link_libraries
            .contains(&"hello_library".to_string())
    );
}

#[test]
fn test_third_party_library() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../test-fixtures/cmake-examples/01-basic/H-third-party-library/CMakeLists.txt");

    if !fixture.exists() {
        eprintln!("Skipping test: fixture not found at {}", fixture.display());
        return;
    }

    let src = std::fs::read_to_string(&fixture).unwrap();
    let file = rebaze_cmake::parse_source(&src).unwrap();

    // Should parse conditionals (if/else/endif)
    let cmd_names: Vec<&str> = file.commands.iter().map(|c| c.name.as_str()).collect();
    assert!(cmd_names.contains(&"cmake_minimum_required"));
    assert!(cmd_names.contains(&"project"));
    assert!(cmd_names.contains(&"find_package"));
    assert!(cmd_names.contains(&"if"));
    assert!(cmd_names.contains(&"else"));
    assert!(cmd_names.contains(&"endif"));
    assert!(cmd_names.contains(&"add_executable"));
    assert!(cmd_names.contains(&"target_link_libraries"));
}

#[test]
fn test_project_extraction_with_boost() {
    let fixture_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../test-fixtures/cmake-examples/01-basic/H-third-party-library");

    if !fixture_dir.exists() {
        eprintln!("Skipping test: fixture not found");
        return;
    }

    let project = rebaze_cmake::parse(&fixture_dir).unwrap();

    assert_eq!(project.name, "third_party_include");
    assert_eq!(project.executables.len(), 1);
    assert_eq!(project.executables[0].name, "third_party_include");

    // Check Boost package was detected
    assert_eq!(project.packages.len(), 1);
    assert_eq!(project.packages[0].name, "Boost");
    assert!(project.packages[0].required);
    assert!(
        project.packages[0]
            .components
            .contains(&"filesystem".to_string())
    );
    assert!(
        project.packages[0]
            .components
            .contains(&"system".to_string())
    );

    // Check link libraries include Boost::filesystem
    assert!(
        project.executables[0]
            .link_libraries
            .contains(&"Boost::filesystem".to_string())
    );
}

#[test]
fn test_recursive_subdirectory_parsing_microdnf() {
    // This test requires microdnf to be cloned at /tmp/microdnf
    let fixture_dir = Path::new("/tmp/microdnf");

    if !fixture_dir.exists() {
        eprintln!("Skipping test: microdnf not found at /tmp/microdnf");
        return;
    }

    let project = rebaze_cmake::extract_project_from_path(fixture_dir).unwrap();

    // Root project info
    assert_eq!(project.name, "microdnf");
    assert_eq!(project.cmake_minimum_version, Some("3.10".to_string()));

    // The microdnf executable is defined in dnf/CMakeLists.txt
    // It should be found via recursive parsing
    assert!(
        project.executables.iter().any(|e| e.name == "microdnf"),
        "Expected to find microdnf executable via recursive parsing. Found: {:?}",
        project
            .executables
            .iter()
            .map(|e| &e.name)
            .collect::<Vec<_>>()
    );

    // The root CMakeLists.txt has add_subdirectory(dnf)
    assert!(
        project.subdirectories.contains(&"dnf".to_string()),
        "Expected subdirectories to contain 'dnf'. Found: {:?}",
        project.subdirectories
    );
}
