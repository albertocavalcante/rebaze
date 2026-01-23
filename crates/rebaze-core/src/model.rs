//! Domain models for build system representation.

use serde::{Deserialize, Serialize};

/// A parsed project, build-system agnostic.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub name: String,
    pub modules: Vec<Module>,
}

/// A module/subproject within a project.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Module {
    pub name: String,
    pub path: String,
    pub targets: Vec<Target>,
    pub dependencies: Vec<Dependency>,
}

/// A build target (library, binary, test).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Target {
    pub name: String,
    pub kind: TargetKind,
    pub sources: Vec<String>,
    pub resources: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum TargetKind {
    Library,
    Binary,
    Test,
}

/// A dependency on another module or external artifact.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Dependency {
    pub name: String,
    pub kind: DependencyKind,
    pub scope: DependencyScope,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum DependencyKind {
    /// Internal module dependency
    Module(String),
    /// External Maven/Gradle artifact
    Maven {
        group: String,
        artifact: String,
        version: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum DependencyScope {
    Compile,
    Runtime,
    Test,
}
