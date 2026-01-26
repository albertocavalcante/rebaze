//! CMake File API integration for rebaze.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use cmake_file_api::{objects, query, reply};
use rebaze_cmake::{CMakeProject, Executable, Library, LibraryKind};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum FileApiError {
    #[error(
        "cmake-file-api reply not found in build dir {build_dir}. Run `cmake -S <src> -B <build>` to generate it."
    )]
    NotGenerated { build_dir: PathBuf },

    #[error("cmake-file-api has no configurations")]
    MissingConfiguration,

    #[error("cmake-file-api configuration '{name}' not found")]
    UnknownConfiguration { name: String },

    #[error("cmake-file-api query error: {0}")]
    Query(#[from] query::WriterError),

    #[error("cmake-file-api read error: {0}")]
    Reader(#[from] reply::ReaderError),
}

#[derive(Debug, Default, Clone)]
pub struct FileApiOptions {
    pub configuration: Option<String>,
}

/// Write a codemodel-v2 query into the build directory.
pub fn write_query(build_dir: &Path) -> Result<(), FileApiError> {
    query::Writer::default()
        .request_object::<objects::CodeModelV2>()
        .write_stateless(build_dir)?;
    Ok(())
}

/// Parse cmake-file-api from a build directory.
pub fn parse_build_dir(build_dir: &Path) -> Result<CMakeProject, FileApiError> {
    parse_build_dir_with_options(build_dir, &FileApiOptions::default())
}

/// Parse cmake-file-api from a build directory with options.
pub fn parse_build_dir_with_options(
    build_dir: &Path,
    options: &FileApiOptions,
) -> Result<CMakeProject, FileApiError> {
    let reader = reply::Reader::from_build_dir(build_dir)
        .map_err(|err| map_reader_error(err, build_dir))?;
    let codemodel: objects::CodeModelV2 = reader.read_object()?;
    project_from_codemodel(&codemodel, options)
}

/// Convert a codemodel into a rebaze CMake project model.
pub fn project_from_codemodel(
    codemodel: &objects::CodeModelV2,
    options: &FileApiOptions,
) -> Result<CMakeProject, FileApiError> {
    let config = select_configuration(codemodel, options.configuration.as_deref())?;
    let source_root = codemodel.paths.source.clone();
    let build_root = codemodel.paths.build.clone();

    let project_name = config
        .projects
        .first()
        .map(|project| project.name.clone())
        .or_else(|| source_root.file_name().map(|name| name.to_string_lossy().to_string()))
        .unwrap_or_else(|| "cmake_project".to_string());

    let mut project = CMakeProject {
        name: project_name,
        path: source_root.clone(),
        ..Default::default()
    };

    if let Some(dir_ref) = config.directory_refs.first() {
        if let Some(min) = &dir_ref.minimum_cmake_version {
            project.cmake_minimum_version = Some(min.version.clone());
        }
    }

    let mut languages = HashSet::new();
    for target in &config.targets {
        for group in &target.compile_groups {
            if !group.language.is_empty() {
                languages.insert(group.language.clone());
            }
        }
    }
    if !languages.is_empty() {
        let mut list: Vec<String> = languages.into_iter().collect();
        list.sort();
        project.languages = list;
    }

    let mut subdirs = HashSet::new();
    for dir_ref in &config.directory_refs {
        let dir = dir_ref.source.to_string_lossy().to_string();
        if dir != "." && !dir.is_empty() {
            subdirs.insert(dir);
        }
    }
    project.subdirectories = sorted_vec(subdirs);

    let mut id_to_name = HashMap::new();
    for target in &config.targets {
        if let Some(kind) = target_kind(target) {
            id_to_name.insert(target.id.clone(), (target.name.clone(), kind));
        }
    }

    for target in &config.targets {
        let Some(kind) = target_kind(target) else {
            continue;
        };

        match kind {
            TargetKind::Executable => {
                let exe = Executable {
                    name: target.name.clone(),
                    sources: collect_sources(target, &source_root, &build_root),
                    link_libraries: collect_dependencies(target, &id_to_name),
                    include_directories: collect_includes(target, &source_root, &build_root),
                    compile_definitions: collect_defines(target),
                    compile_options: collect_compile_options(target),
                };
                project.executables.push(exe);
            }
            TargetKind::Library(lib_kind) => {
                let lib = Library {
                    name: target.name.clone(),
                    kind: lib_kind,
                    sources: collect_sources(target, &source_root, &build_root),
                    link_libraries: collect_dependencies(target, &id_to_name),
                    include_directories: collect_includes(target, &source_root, &build_root),
                    compile_definitions: collect_defines(target),
                    compile_options: collect_compile_options(target),
                };
                project.libraries.push(lib);
            }
        }
    }

    Ok(project)
}

fn map_reader_error(err: reply::ReaderError, build_dir: &Path) -> FileApiError {
    match err {
        reply::ReaderError::FileApiNotGenerated => FileApiError::NotGenerated {
            build_dir: build_dir.to_path_buf(),
        },
        _ => FileApiError::Reader(err),
    }
}

fn select_configuration<'a>(
    codemodel: &'a objects::CodeModelV2,
    config: Option<&str>,
) -> Result<&'a objects::codemodel_v2::Configuration, FileApiError> {
    if codemodel.configurations.is_empty() {
        return Err(FileApiError::MissingConfiguration);
    }

    if let Some(name) = config {
        let name = name.to_string();
        return codemodel
            .configurations
            .iter()
            .find(|cfg| cfg.name.eq_ignore_ascii_case(&name))
            .ok_or(FileApiError::UnknownConfiguration { name });
    }

    Ok(&codemodel.configurations[0])
}

fn collect_sources(
    target: &objects::codemodel_v2::Target,
    source_root: &Path,
    build_root: &Path,
) -> Vec<String> {
    let mut set = HashSet::new();
    for source in &target.sources {
        if source.is_generated {
            continue;
        }
        if let Some(path) = normalize_source_path(&source.path, source_root, build_root) {
            set.insert(path);
        }
    }
    sorted_vec(set)
}

fn collect_includes(
    target: &objects::codemodel_v2::Target,
    source_root: &Path,
    build_root: &Path,
) -> Vec<String> {
    let mut set = HashSet::new();
    for group in &target.compile_groups {
        for include in &group.includes {
            if include.is_system {
                continue;
            }
            if let Some(path) = normalize_include_path(&include.path, source_root, build_root) {
                set.insert(path);
            }
        }
    }
    sorted_vec(set)
}

fn collect_defines(target: &objects::codemodel_v2::Target) -> Vec<String> {
    let mut set = HashSet::new();
    for group in &target.compile_groups {
        for define in group.defines() {
            if !define.is_empty() {
                set.insert(define);
            }
        }
    }
    sorted_vec(set)
}

fn collect_compile_options(target: &objects::codemodel_v2::Target) -> Vec<String> {
    let mut set = HashSet::new();
    for group in &target.compile_groups {
        let mut skip_next = false;
        for flag in group.flags() {
            if skip_next {
                skip_next = false;
                continue;
            }
            if flag == "-I" || flag == "-isystem" {
                skip_next = true;
                continue;
            }
            if flag.starts_with("-I") || flag.starts_with("-isystem") {
                continue;
            }
            if !flag.is_empty() {
                set.insert(flag);
            }
        }
    }
    sorted_vec(set)
}

fn collect_dependencies(
    target: &objects::codemodel_v2::Target,
    id_to_name: &HashMap<String, (String, TargetKind)>,
) -> Vec<String> {
    let mut set = HashSet::new();
    for dep in &target.dependencies {
        if let Some((name, _)) = id_to_name.get(&dep.id) {
            if name != &target.name {
                set.insert(name.clone());
            }
        }
    }
    sorted_vec(set)
}

fn normalize_source_path(path: &Path, source_root: &Path, build_root: &Path) -> Option<String> {
    if path.is_absolute() {
        if path.starts_with(build_root) {
            return None;
        }
        if let Ok(rel) = path.strip_prefix(source_root) {
            return Some(path_to_string(rel));
        }
        return None;
    }

    Some(path_to_string(path))
}

fn normalize_include_path(path: &Path, source_root: &Path, build_root: &Path) -> Option<String> {
    if path.is_absolute() {
        if path.starts_with(build_root) {
            return None;
        }
        if let Ok(rel) = path.strip_prefix(source_root) {
            let value = path_to_string(rel);
            if value.is_empty() || value == "." {
                return None;
            }
            return Some(value);
        }
        return Some(path_to_string(path));
    }

    let value = path_to_string(path);
    if value.is_empty() || value == "." {
        None
    } else {
        Some(value)
    }
}

fn path_to_string(path: &Path) -> String {
    let s = path.to_string_lossy().replace('\\', "/");
    // Collapse multiple slashes and remove leading ./
    let mut result = s.trim_start_matches("./").to_string();
    while result.contains("//") {
        result = result.replace("//", "/");
    }
    result
}

fn sorted_vec(mut set: HashSet<String>) -> Vec<String> {
    let mut list: Vec<String> = set.drain().collect();
    list.sort();
    list
}

#[derive(Debug, Clone, Copy)]
enum TargetKind {
    Executable,
    Library(LibraryKind),
}

fn target_kind(target: &objects::codemodel_v2::Target) -> Option<TargetKind> {
    if target.is_generator_provided {
        return None;
    }

    match target.type_name.as_str() {
        "EXECUTABLE" => Some(TargetKind::Executable),
        "STATIC_LIBRARY" => Some(TargetKind::Library(LibraryKind::Static)),
        "SHARED_LIBRARY" => Some(TargetKind::Library(LibraryKind::Shared)),
        "MODULE_LIBRARY" => Some(TargetKind::Library(LibraryKind::Module)),
        "OBJECT_LIBRARY" => Some(TargetKind::Library(LibraryKind::Object)),
        "INTERFACE_LIBRARY" => Some(TargetKind::Library(LibraryKind::Interface)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cmake_file_api::objects::codemodel_v2::{
        CompileCommandFragment, CompileGroup, Configuration, Dependency, DirectoryReference,
        Include, MinimumCmakeVersion, Project, Target, TargetPaths, TargetReference,
    };
    use cmake_file_api::objects::{MajorMinor, ObjectKind};

    #[test]
    fn test_project_from_codemodel() {
        let source_root = PathBuf::from("/repo");
        let build_root = PathBuf::from("/repo/build");

        let mut lib_paths = TargetPaths::default();
        lib_paths.source = PathBuf::from(".");
        lib_paths.build = PathBuf::from(".");

        let mut lib_source = objects::codemodel_v2::Source::default();
        lib_source.path = PathBuf::from("/repo/src/lib.cpp");

        let mut include = Include::default();
        include.path = PathBuf::from("/repo/include");

        let mut compile_group = CompileGroup::default();
        compile_group.language = "CXX".to_string();
        compile_group.includes = vec![include];
        let mut fragment = CompileCommandFragment::default();
        fragment.fragment = "-O2 -DDEBUG -I/ignored -isystem /sys".to_string();
        compile_group.compile_command_fragments = vec![fragment];

        let mut lib_target = Target::default();
        lib_target.name = "mylib".to_string();
        lib_target.id = "lib-id".to_string();
        lib_target.type_name = "STATIC_LIBRARY".to_string();
        lib_target.paths = lib_paths;
        lib_target.sources = vec![lib_source];
        lib_target.compile_groups = vec![compile_group];

        let mut exe_paths = TargetPaths::default();
        exe_paths.source = PathBuf::from(".");
        exe_paths.build = PathBuf::from(".");

        let mut exe_source = objects::codemodel_v2::Source::default();
        exe_source.path = PathBuf::from("/repo/src/main.cpp");

        let mut dep = Dependency::default();
        dep.id = "lib-id".to_string();

        let mut exe_target = Target::default();
        exe_target.name = "myapp".to_string();
        exe_target.id = "app-id".to_string();
        exe_target.type_name = "EXECUTABLE".to_string();
        exe_target.paths = exe_paths;
        exe_target.sources = vec![exe_source];
        exe_target.dependencies = vec![dep];

        let mut min_version = MinimumCmakeVersion::default();
        min_version.version = "3.20".to_string();

        let mut dir_ref = DirectoryReference::default();
        dir_ref.source = PathBuf::from(".");
        dir_ref.build = PathBuf::from(".");
        dir_ref.project_index = 0;
        dir_ref.minimum_cmake_version = Some(min_version);
        dir_ref.json_file = PathBuf::from("dir.json");

        let mut project = Project::default();
        project.name = "demo".to_string();

        let mut lib_ref = TargetReference::default();
        lib_ref.name = "mylib".to_string();
        lib_ref.id = "lib-id".to_string();
        lib_ref.directory_index = 0;
        lib_ref.project_index = 0;
        lib_ref.json_file = PathBuf::from("lib.json");

        let mut app_ref = TargetReference::default();
        app_ref.name = "myapp".to_string();
        app_ref.id = "app-id".to_string();
        app_ref.directory_index = 0;
        app_ref.project_index = 0;
        app_ref.json_file = PathBuf::from("app.json");

        let mut config = Configuration::default();
        config.name = "Debug".to_string();
        config.projects = vec![project];
        config.directory_refs = vec![dir_ref];
        config.target_refs = vec![lib_ref, app_ref];
        config.targets = vec![lib_target, exe_target];
        config.directories = vec![];

        let mut version = MajorMinor::default();
        version.major = 2;
        version.minor = 0;

        let mut codemodel_paths = objects::codemodel_v2::CodemodelPaths::default();
        codemodel_paths.source = source_root.clone();
        codemodel_paths.build = build_root;

        let mut codemodel = objects::CodeModelV2::default();
        codemodel.kind = ObjectKind::CodeModel;
        codemodel.version = version;
        codemodel.paths = codemodel_paths;
        codemodel.configurations = vec![config];

        let project = project_from_codemodel(&codemodel, &FileApiOptions::default()).unwrap();

        assert_eq!(project.name, "demo");
        assert_eq!(project.path, source_root);
        assert_eq!(project.cmake_minimum_version, Some("3.20".to_string()));
        assert_eq!(project.languages, vec!["CXX".to_string()]);
        assert_eq!(project.libraries.len(), 1);
        assert_eq!(project.executables.len(), 1);
        assert_eq!(project.libraries[0].name, "mylib");
        assert_eq!(project.libraries[0].sources, vec!["src/lib.cpp".to_string()]);
        assert_eq!(project.libraries[0].include_directories, vec!["include".to_string()]);
        assert_eq!(
            project.libraries[0].compile_definitions,
            vec!["DEBUG".to_string()]
        );
        assert_eq!(project.libraries[0].compile_options, vec!["-O2".to_string()]);
        assert_eq!(project.executables[0].link_libraries, vec!["mylib".to_string()]);
    }

    #[test]
    fn test_selects_named_configuration() {
        let mut debug_project = Project::default();
        debug_project.name = "debug_proj".to_string();

        let mut release_project = Project::default();
        release_project.name = "release_proj".to_string();

        let mut debug_config = Configuration::default();
        debug_config.name = "Debug".to_string();
        debug_config.projects = vec![debug_project];

        let mut release_config = Configuration::default();
        release_config.name = "Release".to_string();
        release_config.projects = vec![release_project];

        let mut version = MajorMinor::default();
        version.major = 2;
        version.minor = 0;

        let mut paths = objects::codemodel_v2::CodemodelPaths::default();
        paths.source = PathBuf::from("/repo");
        paths.build = PathBuf::from("/repo/build");

        let mut codemodel = objects::CodeModelV2::default();
        codemodel.kind = ObjectKind::CodeModel;
        codemodel.version = version;
        codemodel.paths = paths;
        codemodel.configurations = vec![debug_config, release_config];

        let options = FileApiOptions {
            configuration: Some("Release".to_string()),
        };
        let project = project_from_codemodel(&codemodel, &options).unwrap();

        assert_eq!(project.name, "release_proj");
    }
}
