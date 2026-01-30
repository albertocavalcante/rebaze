//! CMake project model extraction.
//!
//! Interprets parsed CMake commands to build a project model
//! suitable for Bazel migration.

mod commands;
mod extract;
mod path_utils;
mod target_props;
mod types;

// Re-export public types
pub use types::{
    CMakeProject, Executable, ExtractError, Library, LibraryKind, Package, PkgConfigModule,
};

// Re-export main extraction functions
pub use extract::{extract_project, extract_project_from_path, extract_project_with_context};

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::parser;
    use std::path::PathBuf;

    fn parse_and_extract(src: &str) -> CMakeProject {
        let (file, errors) = parser::parse(src);
        assert!(errors.is_empty(), "Parse errors: {errors:?}");
        extract_project(&file.unwrap(), PathBuf::from("."))
    }

    #[test]
    fn test_project_extraction() {
        let project = parse_and_extract(
            "
            cmake_minimum_required(VERSION 3.20)
            project(myapp VERSION 1.0.0 LANGUAGES CXX)
        ",
        );

        assert_eq!(project.name, "myapp");
        assert_eq!(project.version, Some("1.0.0".to_string()));
        assert_eq!(project.languages, vec!["CXX"]);
        assert_eq!(project.cmake_minimum_version, Some("3.20".to_string()));
    }

    #[test]
    fn test_executable_extraction() {
        let project = parse_and_extract(
            "
            project(myapp)
            add_executable(myapp main.cpp util.cpp)
            target_link_libraries(myapp pthread)
        ",
        );

        assert_eq!(project.executables.len(), 1);
        let exe = &project.executables[0];
        assert_eq!(exe.name, "myapp");
        assert_eq!(exe.sources, vec!["main.cpp", "util.cpp"]);
        assert_eq!(exe.link_libraries, vec!["pthread"]);
    }

    #[test]
    fn test_library_extraction() {
        let project = parse_and_extract(
            "
            project(mylib)
            add_library(mylib STATIC lib.cpp)
            add_library(mylib_shared SHARED lib.cpp)
        ",
        );

        assert_eq!(project.libraries.len(), 2);
        assert_eq!(project.libraries[0].kind, LibraryKind::Static);
        assert_eq!(project.libraries[1].kind, LibraryKind::Shared);
    }

    #[test]
    fn test_find_package() {
        let project = parse_and_extract(
            "
            project(myapp)
            find_package(Boost 1.70 REQUIRED COMPONENTS system filesystem)
            find_package(OpenSSL REQUIRED)
        ",
        );

        assert_eq!(project.packages.len(), 2);

        let boost = &project.packages[0];
        assert_eq!(boost.name, "Boost");
        assert_eq!(boost.version, Some("1.70".to_string()));
        assert!(boost.required);
        assert_eq!(boost.components, vec!["system", "filesystem"]);

        let ssl = &project.packages[1];
        assert_eq!(ssl.name, "OpenSSL");
        assert!(ssl.required);
    }

    #[test]
    fn test_pkg_check_modules() {
        let project = parse_and_extract(
            "
            project(microdnf)
            pkg_check_modules(GLIB REQUIRED glib-2.0>=2.44.0)
            pkg_check_modules(LIBDNF REQUIRED libdnf>=0.62.0)
            add_executable(microdnf main.c)
            target_link_libraries(microdnf ${GLIB_LIBRARIES} ${LIBDNF_LIBRARIES})
        ",
        );

        assert_eq!(project.pkg_config_modules.len(), 2);

        let glib = &project.pkg_config_modules[0];
        assert_eq!(glib.prefix, "GLIB");
        assert!(glib.required);
        assert_eq!(glib.packages, vec!["glib-2.0>=2.44.0"]);

        let libdnf = &project.pkg_config_modules[1];
        assert_eq!(libdnf.prefix, "LIBDNF");
        assert!(libdnf.required);
        assert_eq!(libdnf.packages, vec!["libdnf>=0.62.0"]);

        // Check that variables were expanded in link_libraries
        let exe = &project.executables[0];
        assert_eq!(exe.link_libraries, vec!["-lglib-2.0", "-llibdnf"]);
    }

    #[test]
    fn test_compile_flags_extraction() {
        let project = parse_and_extract(
            "
            project(myapp)
            add_library(mylib STATIC lib.cpp)
            add_executable(myapp main.cpp)
            target_compile_definitions(mylib PRIVATE FOO BAR=1 -DBAZ /DWIN32)
            target_compile_options(mylib PUBLIC -O2 -Wall -Iinclude -isystem /sys /Iwin)
            target_compile_definitions(myapp INTERFACE APPDEF)
            target_compile_options(myapp PRIVATE -g)
        ",
        );

        let lib = &project.libraries[0];
        assert_eq!(
            lib.compile_definitions,
            vec!["FOO", "BAR=1", "BAZ", "WIN32"]
        );
        assert_eq!(lib.compile_options, vec!["-O2", "-Wall"]);

        let exe = &project.executables[0];
        assert_eq!(exe.compile_definitions, vec!["APPDEF"]);
        assert_eq!(exe.compile_options, vec!["-g"]);
    }

    #[test]
    fn test_skip_alias_library() {
        let project = parse_and_extract(
            "
            project(myapp)
            add_library(alias_lib ALIAS real_lib)
            add_library(real_lib STATIC lib.cpp)
        ",
        );

        assert_eq!(project.libraries.len(), 1);
        assert_eq!(project.libraries[0].name, "real_lib");
        // Verify alias was tracked
        assert_eq!(project.aliases.len(), 1);
        assert_eq!(
            project.aliases.get("alias_lib"),
            Some(&"real_lib".to_string())
        );
    }

    #[test]
    fn test_alias_with_namespace() {
        let project = parse_and_extract(
            "
            project(mylib)
            add_library(mylib_impl SHARED src/impl.cpp)
            add_library(my::lib ALIAS mylib_impl)
            add_executable(myapp src/main.cpp)
            target_link_libraries(myapp PRIVATE my::lib)
        ",
        );

        // Verify alias was tracked with namespace
        assert_eq!(
            project.aliases.get("my::lib"),
            Some(&"mylib_impl".to_string())
        );
        // Verify executable has the alias in its link_libraries
        let exe = &project.executables[0];
        assert!(exe.link_libraries.contains(&"my::lib".to_string()));
    }

    #[test]
    fn test_skip_dynamic_target_name() {
        let project = parse_and_extract(
            "
            project(myapp)
            add_executable(${PROJECT_NAME} main.cpp)
        ",
        );

        assert!(project.executables.is_empty());
    }

    #[test]
    fn test_variable_expansion_in_sources() {
        let project = parse_and_extract(
            "
            project(microdnf)
            set(DNF_SRCS dnf-command.c dnf-utils.c)
            add_executable(microdnf ${DNF_SRCS})
        ",
        );

        assert_eq!(project.executables.len(), 1);
        let exe = &project.executables[0];
        assert_eq!(exe.name, "microdnf");
        assert_eq!(exe.sources, vec!["dnf-command.c", "dnf-utils.c"]);
    }

    #[test]
    fn test_variable_expansion_with_list_append() {
        let project = parse_and_extract(
            "
            project(mylib)
            set(LIB_SRCS core.cpp)
            list(APPEND LIB_SRCS utils.cpp helper.cpp)
            add_library(mylib STATIC ${LIB_SRCS})
        ",
        );

        assert_eq!(project.libraries.len(), 1);
        let lib = &project.libraries[0];
        assert_eq!(lib.name, "mylib");
        assert_eq!(lib.sources, vec!["core.cpp", "utils.cpp", "helper.cpp"]);
    }

    #[test]
    fn test_variable_expansion_mixed_sources() {
        let project = parse_and_extract(
            "
            project(myapp)
            set(COMMON_SRCS a.c b.c)
            add_executable(myapp main.c ${COMMON_SRCS} extra.c)
        ",
        );

        assert_eq!(project.executables.len(), 1);
        let exe = &project.executables[0];
        assert_eq!(exe.name, "myapp");
        assert_eq!(exe.sources, vec!["main.c", "a.c", "b.c", "extra.c"]);
    }

    #[test]
    fn test_extract_project_from_path() {
        use std::io::Write;

        // Create a temporary directory structure
        let temp_dir = std::env::temp_dir().join("rebaze_test_recursive");
        let _ = std::fs::remove_dir_all(&temp_dir); // Clean up from previous runs
        std::fs::create_dir_all(&temp_dir).unwrap();

        // Create subdirectory
        let sub_dir = temp_dir.join("subdir");
        std::fs::create_dir_all(&sub_dir).unwrap();

        // Write root CMakeLists.txt
        let mut root_cmake = std::fs::File::create(temp_dir.join("CMakeLists.txt")).unwrap();
        writeln!(
            root_cmake,
            r"
cmake_minimum_required(VERSION 3.10)
project(testproject)
add_library(rootlib STATIC root.cpp)
add_subdirectory(subdir)
"
        )
        .unwrap();

        // Write subdirectory CMakeLists.txt
        let mut sub_cmake = std::fs::File::create(sub_dir.join("CMakeLists.txt")).unwrap();
        writeln!(
            sub_cmake,
            r"
add_executable(subapp main.cpp)
add_library(sublib SHARED sub.cpp)
"
        )
        .unwrap();

        // Test recursive extraction
        let project = extract_project_from_path(&temp_dir).unwrap();

        // Verify root project info
        assert_eq!(project.name, "testproject");
        assert_eq!(project.cmake_minimum_version, Some("3.10".to_string()));

        // Verify all targets are merged
        assert_eq!(project.libraries.len(), 2);
        assert_eq!(project.executables.len(), 1);

        // Check library names
        let lib_names: Vec<&str> = project.libraries.iter().map(|l| l.name.as_str()).collect();
        assert!(lib_names.contains(&"rootlib"));
        assert!(lib_names.contains(&"sublib"));

        // Check executable name
        assert_eq!(project.executables[0].name, "subapp");

        // Verify subdirectories are recorded
        assert_eq!(project.subdirectories, vec!["subdir"]);

        // Clean up
        std::fs::remove_dir_all(&temp_dir).unwrap();
    }

    #[test]
    fn test_extract_project_from_path_missing_subdir() {
        use std::io::Write;

        // Create a temporary directory
        let temp_dir = std::env::temp_dir().join("rebaze_test_missing_subdir");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();

        // Write CMakeLists.txt that references a non-existent subdirectory
        let mut root_cmake = std::fs::File::create(temp_dir.join("CMakeLists.txt")).unwrap();
        writeln!(
            root_cmake,
            r"
cmake_minimum_required(VERSION 3.10)
project(testproject)
add_subdirectory(nonexistent)
"
        )
        .unwrap();

        // Should succeed despite missing subdirectory
        let project = extract_project_from_path(&temp_dir).unwrap();
        assert_eq!(project.name, "testproject");
        assert_eq!(project.subdirectories, vec!["nonexistent"]);

        // Clean up
        std::fs::remove_dir_all(&temp_dir).unwrap();
    }

    #[test]
    fn test_extract_project_from_path_nested() {
        use std::io::Write;

        // Create a deeply nested directory structure
        let temp_dir = std::env::temp_dir().join("rebaze_test_nested");
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();

        let level1 = temp_dir.join("level1");
        let level2 = level1.join("level2");
        std::fs::create_dir_all(&level2).unwrap();

        // Root CMakeLists.txt
        let mut root_cmake = std::fs::File::create(temp_dir.join("CMakeLists.txt")).unwrap();
        writeln!(
            root_cmake,
            r"
project(root)
add_executable(root_exe main.cpp)
add_subdirectory(level1)
"
        )
        .unwrap();

        // Level 1 CMakeLists.txt
        let mut level1_cmake = std::fs::File::create(level1.join("CMakeLists.txt")).unwrap();
        writeln!(
            level1_cmake,
            r"
add_executable(level1_exe main.cpp)
add_subdirectory(level2)
"
        )
        .unwrap();

        // Level 2 CMakeLists.txt
        let mut level2_cmake = std::fs::File::create(level2.join("CMakeLists.txt")).unwrap();
        writeln!(
            level2_cmake,
            r"
add_executable(level2_exe main.cpp)
"
        )
        .unwrap();

        // Test recursive extraction
        let project = extract_project_from_path(&temp_dir).unwrap();

        // All 3 executables should be found
        assert_eq!(project.executables.len(), 3);
        let exe_names: Vec<&str> = project
            .executables
            .iter()
            .map(|e| e.name.as_str())
            .collect();
        assert!(exe_names.contains(&"root_exe"));
        assert!(exe_names.contains(&"level1_exe"));
        assert!(exe_names.contains(&"level2_exe"));

        // Clean up
        std::fs::remove_dir_all(&temp_dir).unwrap();
    }

    #[test]
    fn test_global_include_directories() {
        let project = parse_and_extract(
            "
            project(myapp)
            include_directories(include)
            include_directories(${CMAKE_CURRENT_SOURCE_DIR})
            include_directories(${CMAKE_CURRENT_SOURCE_DIR}/src)
            add_executable(myapp main.cpp)
            add_library(mylib STATIC lib.cpp)
        ",
        );

        // Check global include directories were extracted
        assert_eq!(project.global_include_directories.len(), 3);
        assert!(
            project
                .global_include_directories
                .contains(&"include".to_string())
        );
        assert!(
            project
                .global_include_directories
                .contains(&".".to_string())
        );
        assert!(
            project
                .global_include_directories
                .contains(&"src".to_string())
        );

        // Check they were applied to the executable
        let exe = &project.executables[0];
        assert!(exe.include_directories.contains(&"include".to_string()));
        assert!(exe.include_directories.contains(&".".to_string()));
        assert!(exe.include_directories.contains(&"src".to_string()));

        // Check they were applied to the library
        let lib = &project.libraries[0];
        assert!(lib.include_directories.contains(&"include".to_string()));
        assert!(lib.include_directories.contains(&".".to_string()));
        assert!(lib.include_directories.contains(&"src".to_string()));
    }

    #[test]
    fn test_global_include_directories_with_target_specific() {
        let project = parse_and_extract(
            "
            project(myapp)
            include_directories(global_include)
            add_executable(myapp main.cpp)
            target_include_directories(myapp PRIVATE target_include)
        ",
        );

        // Executable should have both global and target-specific includes
        let exe = &project.executables[0];
        assert!(
            exe.include_directories
                .contains(&"target_include".to_string())
        );
        assert!(
            exe.include_directories
                .contains(&"global_include".to_string())
        );
    }

    #[test]
    fn test_global_include_directories_skip_unexpanded() {
        let project = parse_and_extract(
            "
            project(myapp)
            include_directories(${SOME_UNKNOWN_VAR})
            include_directories(valid_dir)
            add_executable(myapp main.cpp)
        ",
        );

        // Should only have valid_dir, not the unexpanded variable
        assert_eq!(project.global_include_directories.len(), 1);
        assert!(
            project
                .global_include_directories
                .contains(&"valid_dir".to_string())
        );
    }

    #[test]
    fn test_target_include_directories_normalizes_project_source_dir() {
        let project = parse_and_extract(
            "
            project(mylib)
            add_library(mylib STATIC lib.cpp)
            target_include_directories(mylib PUBLIC ${PROJECT_SOURCE_DIR}/include)
            target_include_directories(mylib PRIVATE ${CMAKE_SOURCE_DIR}/src)
        ",
        );

        assert_eq!(project.libraries.len(), 1);
        let lib = &project.libraries[0];
        // PROJECT_SOURCE_DIR/include should normalize to just "include"
        assert!(
            lib.include_directories.contains(&"include".to_string()),
            "Expected 'include' but got: {:?}",
            lib.include_directories
        );
        // CMAKE_SOURCE_DIR/src should normalize to just "src"
        assert!(
            lib.include_directories.contains(&"src".to_string()),
            "Expected 'src' but got: {:?}",
            lib.include_directories
        );
        // Should NOT have leading slashes
        assert!(
            !lib.include_directories.iter().any(|d| d.starts_with('/')),
            "Include directories should not have leading slashes: {:?}",
            lib.include_directories
        );
    }

    #[test]
    fn test_target_include_directories_strips_leading_slash() {
        // This tests the case where a CMake variable expands to empty, leaving "/subdir"
        let project = parse_and_extract(
            "
            project(mylib)
            add_library(mylib STATIC lib.cpp)
            target_include_directories(mylib PUBLIC /include)
        ",
        );

        assert_eq!(project.libraries.len(), 1);
        let lib = &project.libraries[0];
        // "/include" should normalize to "include"
        assert!(
            lib.include_directories.contains(&"include".to_string()),
            "Expected 'include' but got: {:?}",
            lib.include_directories
        );
    }

    #[test]
    fn test_target_sources() {
        let project = parse_and_extract(
            "
            project(mylib)
            add_library(mylib STATIC initial.cpp)
            target_sources(mylib PRIVATE extra1.cpp extra2.cpp)
            target_sources(mylib PUBLIC public.cpp)
        ",
        );

        assert_eq!(project.libraries.len(), 1);
        let lib = &project.libraries[0];
        assert_eq!(lib.name, "mylib");
        assert_eq!(
            lib.sources,
            vec!["initial.cpp", "extra1.cpp", "extra2.cpp", "public.cpp"]
        );
    }

    #[test]
    fn test_target_sources_with_file_set() {
        let project = parse_and_extract(
            "
            project(mylib)
            add_library(mylib STATIC initial.cpp)
            target_sources(mylib
                PUBLIC
                    FILE_SET HEADERS
                    TYPE HEADERS
                    BASE_DIRS include
                    FILES include/mylib.h
                PRIVATE
                    impl.cpp
            )
        ",
        );

        assert_eq!(project.libraries.len(), 1);
        let lib = &project.libraries[0];
        // FILE_SET block should be skipped, only impl.cpp should be captured
        assert_eq!(lib.sources, vec!["initial.cpp", "impl.cpp"]);
    }
}
