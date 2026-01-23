//! CMake project model extraction.
//!
//! Interprets parsed CMake commands to build a project model
//! suitable for Bazel migration.

use std::path::PathBuf;

use crate::ast::{Argument, CMakeFile, Command};

/// A parsed CMake project.
#[derive(Debug, Clone, Default)]
pub struct CMakeProject {
    /// Project name from project() command.
    pub name: String,
    /// Project version if specified.
    pub version: Option<String>,
    /// Languages used (C, CXX, etc.).
    pub languages: Vec<String>,
    /// Minimum CMake version required.
    pub cmake_minimum_version: Option<String>,
    /// Root path of the project.
    pub path: PathBuf,
    /// Executable targets.
    pub executables: Vec<Executable>,
    /// Library targets.
    pub libraries: Vec<Library>,
    /// External package dependencies.
    pub packages: Vec<Package>,
    /// Subdirectories (add_subdirectory calls).
    pub subdirectories: Vec<String>,
}

/// An executable target.
#[derive(Debug, Clone)]
pub struct Executable {
    pub name: String,
    pub sources: Vec<String>,
    pub link_libraries: Vec<String>,
    pub include_directories: Vec<String>,
}

/// A library target.
#[derive(Debug, Clone)]
pub struct Library {
    pub name: String,
    pub kind: LibraryKind,
    pub sources: Vec<String>,
    pub link_libraries: Vec<String>,
    pub include_directories: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LibraryKind {
    Static,
    Shared,
    Module,
    Object,
    Interface,
    Unknown,
}

/// An external package dependency.
#[derive(Debug, Clone)]
pub struct Package {
    pub name: String,
    pub version: Option<String>,
    pub required: bool,
    pub components: Vec<String>,
}

/// Extract project information from a parsed CMake file.
pub fn extract_project(file: &CMakeFile, path: PathBuf) -> CMakeProject {
    let mut project = CMakeProject {
        path,
        ..Default::default()
    };

    // First pass: extract basic project info and targets
    for cmd in &file.commands {
        match cmd.name.as_str() {
            "cmake_minimum_required" => extract_cmake_version(cmd, &mut project),
            "project" => extract_project_info(cmd, &mut project),
            "add_executable" => extract_executable(cmd, &mut project),
            "add_library" => extract_library(cmd, &mut project),
            "find_package" => extract_package(cmd, &mut project),
            "add_subdirectory" => extract_subdirectory(cmd, &mut project),
            _ => {}
        }
    }

    // Second pass: attach target properties
    for cmd in &file.commands {
        match cmd.name.as_str() {
            "target_link_libraries" => apply_link_libraries(cmd, &mut project),
            "target_include_directories" => apply_include_directories(cmd, &mut project),
            _ => {}
        }
    }

    project
}

fn extract_cmake_version(cmd: &Command, project: &mut CMakeProject) {
    // cmake_minimum_required(VERSION x.y.z)
    let args = cmd.args_literals();
    for (i, arg) in args.iter().enumerate() {
        if arg.eq_ignore_ascii_case("VERSION") {
            if let Some(version) = args.get(i + 1) {
                project.cmake_minimum_version = Some((*version).to_string());
                return;
            }
        }
    }
}

fn extract_project_info(cmd: &Command, project: &mut CMakeProject) {
    // project(name [VERSION x.y.z] [LANGUAGES lang1 lang2...])
    let args = cmd.args_literals();
    if args.is_empty() {
        return;
    }

    project.name = args[0].to_string();

    let mut i = 1;
    while i < args.len() {
        match args[i].to_uppercase().as_str() {
            "VERSION" => {
                if let Some(ver) = args.get(i + 1) {
                    project.version = Some((*ver).to_string());
                }
                i += 2;
            }
            "LANGUAGES" => {
                i += 1;
                while i < args.len() && !is_keyword(args[i]) {
                    project.languages.push(args[i].to_string());
                    i += 1;
                }
            }
            _ => i += 1,
        }
    }

    // Default to C and CXX if no languages specified
    if project.languages.is_empty() {
        project.languages = vec!["C".to_string(), "CXX".to_string()];
    }
}

fn extract_executable(cmd: &Command, project: &mut CMakeProject) {
    // add_executable(name [WIN32] [MACOSX_BUNDLE] source1 source2...)
    let args = cmd.args_literals();
    if args.is_empty() {
        return;
    }

    let name = args[0].to_string();
    let sources: Vec<String> = args[1..]
        .iter()
        .filter(|s| !matches!(s.to_uppercase().as_str(), "WIN32" | "MACOSX_BUNDLE" | "EXCLUDE_FROM_ALL"))
        .map(|s| (*s).to_string())
        .collect();

    project.executables.push(Executable {
        name,
        sources,
        link_libraries: Vec::new(),
        include_directories: Vec::new(),
    });
}

fn extract_library(cmd: &Command, project: &mut CMakeProject) {
    // add_library(name [STATIC|SHARED|MODULE|OBJECT|INTERFACE] source1 source2...)
    let args = cmd.args_literals();
    if args.is_empty() {
        return;
    }

    let name = args[0].to_string();
    let mut kind = LibraryKind::Unknown;
    let mut sources = Vec::new();

    for arg in &args[1..] {
        match arg.to_uppercase().as_str() {
            "STATIC" => kind = LibraryKind::Static,
            "SHARED" => kind = LibraryKind::Shared,
            "MODULE" => kind = LibraryKind::Module,
            "OBJECT" => kind = LibraryKind::Object,
            "INTERFACE" => kind = LibraryKind::Interface,
            "EXCLUDE_FROM_ALL" | "IMPORTED" | "ALIAS" => {}
            _ => sources.push((*arg).to_string()),
        }
    }

    project.libraries.push(Library {
        name,
        kind,
        sources,
        link_libraries: Vec::new(),
        include_directories: Vec::new(),
    });
}

fn extract_package(cmd: &Command, project: &mut CMakeProject) {
    // find_package(PackageName [version] [REQUIRED] [COMPONENTS comp1...])
    let args = cmd.args_literals();
    if args.is_empty() {
        return;
    }

    let name = args[0].to_string();
    let mut version = None;
    let mut required = false;
    let mut components = Vec::new();
    let mut in_components = false;

    for (i, arg) in args[1..].iter().enumerate() {
        let upper = arg.to_uppercase();
        match upper.as_str() {
            "REQUIRED" => required = true,
            "COMPONENTS" | "OPTIONAL_COMPONENTS" => in_components = true,
            "CONFIG" | "MODULE" | "NO_MODULE" | "QUIET" => in_components = false,
            _ => {
                if in_components {
                    components.push((*arg).to_string());
                } else if i == 0 && !is_keyword(arg) {
                    // First non-keyword arg after name might be version
                    version = Some((*arg).to_string());
                }
            }
        }
    }

    project.packages.push(Package {
        name,
        version,
        required,
        components,
    });
}

fn extract_subdirectory(cmd: &Command, project: &mut CMakeProject) {
    if let Some(dir) = cmd.arg_literal(0) {
        project.subdirectories.push(dir.to_string());
    }
}

fn apply_link_libraries(cmd: &Command, project: &mut CMakeProject) {
    // target_link_libraries(target [PUBLIC|PRIVATE|INTERFACE] lib1 lib2...)
    let args = cmd.args_literals();
    if args.len() < 2 {
        return;
    }

    let target = args[0];
    let libs: Vec<String> = args[1..]
        .iter()
        .filter(|s| !matches!(s.to_uppercase().as_str(), "PUBLIC" | "PRIVATE" | "INTERFACE"))
        .map(|s| (*s).to_string())
        .collect();

    // Find and update the target
    for exe in &mut project.executables {
        if exe.name == target {
            exe.link_libraries.extend(libs);
            return;
        }
    }
    for lib in &mut project.libraries {
        if lib.name == target {
            lib.link_libraries.extend(libs);
            return;
        }
    }
}

fn apply_include_directories(cmd: &Command, project: &mut CMakeProject) {
    // target_include_directories(target [PUBLIC|PRIVATE|INTERFACE] dir1 dir2...)
    let args: Vec<&str> = cmd.arguments.iter().filter_map(Argument::as_literal).collect();
    if args.len() < 2 {
        return;
    }

    let target = args[0];
    let dirs: Vec<String> = args[1..]
        .iter()
        .filter(|s| !matches!(s.to_uppercase().as_str(), "PUBLIC" | "PRIVATE" | "INTERFACE" | "SYSTEM" | "BEFORE" | "AFTER"))
        .map(|s| (*s).to_string())
        .collect();

    for exe in &mut project.executables {
        if exe.name == target {
            exe.include_directories.extend(dirs);
            return;
        }
    }
    for lib in &mut project.libraries {
        if lib.name == target {
            lib.include_directories.extend(dirs);
            return;
        }
    }
}

fn is_keyword(s: &str) -> bool {
    matches!(
        s.to_uppercase().as_str(),
        "VERSION" | "LANGUAGES" | "DESCRIPTION" | "HOMEPAGE_URL"
    )
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::parser;

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
}
