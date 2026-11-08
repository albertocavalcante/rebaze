//! Type definitions for CMake project model.
//!
//! Contains the core types representing CMake projects, targets,
//! and their properties extracted from CMakeLists.txt files.

use std::path::PathBuf;

/// Error type for project extraction.
#[derive(Debug, thiserror::Error)]
pub enum ExtractError {
    #[error(transparent)]
    Input(#[from] rebaze_input::ReadError),

    /// Failed to read a CMakeLists.txt file.
    #[error("Failed to read {path}: {source}")]
    ReadFile {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// Failed to canonicalize a path.
    #[error("Failed to canonicalize path {path}: {source}")]
    Canonicalize {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// CMakeLists.txt not found.
    #[error("CMakeLists.txt not found at {0}")]
    NotFound(PathBuf),

    #[error("repeated or cyclic source directory requires CMake evaluation: {0}")]
    RepeatedDirectory(PathBuf),

    /// Parse error in CMakeLists.txt.
    #[error("Parse error in {path}: {message}")]
    Parse { path: PathBuf, message: String },
}

/// A parsed CMake project.
#[derive(Debug, Clone, Default)]
pub struct CMakeProject {
    /// Source constructs requiring evaluated CMake evidence before native migration.
    pub requires_evaluation: Vec<SourceIssue>,
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
    /// pkg-config modules (from pkg_check_modules).
    pub pkg_config_modules: Vec<PkgConfigModule>,
    /// Subdirectories (add_subdirectory calls).
    pub subdirectories: Vec<String>,
    /// Global include directories (from include_directories() commands).
    pub global_include_directories: Vec<String>,
    /// C++ standard version detected (e.g., "11", "14", "17", "20", "23").
    /// Extracted from CMAKE_CXX_STANDARD, target_compile_features, or compile flags.
    pub cxx_standard: Option<String>,
    /// C standard version detected (e.g., "99", "11", "17", "23").
    pub c_standard: Option<String>,
    /// Observed alias declarations, retaining duplicates for validation.
    /// Created by add_library(alias_name ALIAS real_target) commands.
    pub aliases: Vec<TargetAlias>,
    /// Explicit test registrations; native migration validates its supported subset.
    pub testing: Testing,
}

/// An observed source construct that the static migration profile cannot evaluate.
#[derive(Debug, Clone, thiserror::Error)]
#[error("{path} at character {offset}: {command} requires CMake evaluation: {reason}")]
pub struct SourceIssue {
    pub path: PathBuf,
    pub offset: usize,
    pub command: String,
    pub reason: EvaluationReason,
}

#[derive(Debug, Clone, Copy, thiserror::Error)]
pub enum EvaluationReason {
    #[error("conditional, loop, function, macro or block scope is unsupported")]
    ControlFlow,
    #[error("command is outside the supported source profile")]
    Command,
    #[error("environment, cache, generator, nested or undefined-variable expansion is unsupported")]
    Expansion,
    #[error("semicolon lists and quoted variable expansion require CMake argument modeling")]
    ArgumentList,
    #[error(
        "cache/environment/parent-scope assignments and dynamic variable names are unsupported"
    )]
    Assignment,
    #[error(
        "only list(APPEND ...) and list(PREPEND ...) with a literal variable name are supported"
    )]
    List,
    #[error("add_subdirectory requires one literal relative source directory")]
    Subdirectory,
    #[error("target properties require an existing target declared in the same directory")]
    TargetScope,
    #[error("different target/directory language standards need per-target compiler modeling")]
    LanguageStandard,
    #[error("inherited include_directories needs directory-relative path modeling")]
    DirectoryIncludes,
    #[error("metadata and target names must be literal")]
    Name,
    #[error("subdirectories were not read; use extract_project_from_path for recursive extraction")]
    UnvisitedSubdirectory,
    #[error(
        "package/module evaluation can mutate source state; select configured File API evidence"
    )]
    Module,
}

/// An observed alias declaration, before identity/graph validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetAlias {
    pub name: String,
    pub target: String,
}

/// Test intent from source files, independent of installed build tools.
#[derive(Debug, Clone, Default)]
pub struct Testing {
    /// An unconditional `enable_testing()` in this directory.
    pub enabled: bool,
    pub tests: Vec<CMakeTest>,
    /// Test semantics which extraction cannot faithfully represent.
    pub unsupported: Vec<String>,
    /// Settings requiring evaluation if this project or its children register tests.
    pub requires_evaluation: Vec<TestEvaluation>,
}

/// Source settings whose effect on tests requires CMake evaluation.
#[derive(Debug, Clone)]
pub enum TestEvaluation {
    Launcher,
    ScopedSubdirectory,
    DynamicProperty,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CMakeTest {
    pub name: String,
    pub executable: String,
    pub args: Vec<String>,
}

/// An executable target.
#[derive(Debug, Clone)]
pub struct Executable {
    pub name: String,
    pub sources: Vec<String>,
    pub link_libraries: Vec<String>,
    pub include_directories: Vec<String>,
    pub compile_definitions: Vec<String>,
    pub compile_options: Vec<String>,
}

impl Executable {
    /// Create a new executable with just a name.
    pub const fn new(name: String) -> Self {
        Self {
            name,
            sources: Vec::new(),
            link_libraries: Vec::new(),
            include_directories: Vec::new(),
            compile_definitions: Vec::new(),
            compile_options: Vec::new(),
        }
    }

    /// Create a new executable with name and sources.
    pub const fn with_sources(name: String, sources: Vec<String>) -> Self {
        Self {
            name,
            sources,
            link_libraries: Vec::new(),
            include_directories: Vec::new(),
            compile_definitions: Vec::new(),
            compile_options: Vec::new(),
        }
    }
}

/// A library target.
#[derive(Debug, Clone)]
pub struct Library {
    pub name: String,
    pub kind: LibraryKind,
    pub sources: Vec<String>,
    pub link_libraries: Vec<String>,
    pub include_directories: Vec<String>,
    pub compile_definitions: Vec<String>,
    pub compile_options: Vec<String>,
}

impl Library {
    /// Create a new library with name and kind.
    pub const fn new(name: String, kind: LibraryKind) -> Self {
        Self {
            name,
            kind,
            sources: Vec::new(),
            link_libraries: Vec::new(),
            include_directories: Vec::new(),
            compile_definitions: Vec::new(),
            compile_options: Vec::new(),
        }
    }

    /// Create a new library with name, kind, and sources.
    pub const fn with_sources(name: String, kind: LibraryKind, sources: Vec<String>) -> Self {
        Self {
            name,
            kind,
            sources,
            link_libraries: Vec::new(),
            include_directories: Vec::new(),
            compile_definitions: Vec::new(),
            compile_options: Vec::new(),
        }
    }
}

/// The type of library target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LibraryKind {
    Static,
    Shared,
    Module,
    Object,
    Interface,
    #[default]
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

/// A pkg-config module dependency.
#[derive(Debug, Clone)]
pub struct PkgConfigModule {
    /// The CMake variable prefix (e.g., "GLIB")
    pub prefix: String,
    /// The pkg-config package names (e.g., `["glib-2.0"]`)
    pub packages: Vec<String>,
    /// Whether this package is required
    pub required: bool,
}
