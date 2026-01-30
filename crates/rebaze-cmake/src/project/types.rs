//! Type definitions for CMake project model.
//!
//! Contains the core types representing CMake projects, targets,
//! and their properties extracted from CMakeLists.txt files.

use std::collections::HashMap;
use std::path::PathBuf;

/// Error type for project extraction.
#[derive(Debug, thiserror::Error)]
pub enum ExtractError {
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

    /// Parse error in CMakeLists.txt.
    #[error("Parse error in {path}: {message}")]
    Parse { path: PathBuf, message: String },
}

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
    /// Target aliases (alias_name -> real_target_name).
    /// Created by add_library(alias_name ALIAS real_target) commands.
    pub aliases: HashMap<String, String>,
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
    pub fn new(name: String) -> Self {
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
    pub fn with_sources(name: String, sources: Vec<String>) -> Self {
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
    pub fn new(name: String, kind: LibraryKind) -> Self {
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
    pub fn with_sources(name: String, kind: LibraryKind, sources: Vec<String>) -> Self {
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
