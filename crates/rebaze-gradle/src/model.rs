use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct GradleProject {
    pub name: String,
    pub path: PathBuf,
    pub modules: Vec<GradleModule>,
    /// Existing Bazel Maven lock, supplied explicitly by the input project.
    pub maven_lock: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct GradleModule {
    /// Empty for the root, otherwise a canonical colon-separated project path.
    pub path: String,
    pub plugins: Vec<GradlePlugin>,
    pub dependencies: Vec<Dependency>,
    pub maven_central: bool,
    pub sources: Vec<String>,
    pub resources: Vec<String>,
    pub test_sources: Vec<String>,
    pub test_resources: Vec<String>,
    pub test_classes: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct GradlePlugin {
    pub id: String,
    pub version: Option<String>,
}

#[derive(Debug, Clone)]
pub enum Dependency {
    Maven(GradleDependency),
    Project { configuration: String, path: String },
}

impl Dependency {
    pub fn configuration(&self) -> &str {
        match self {
            Self::Maven(dep) => &dep.configuration,
            Self::Project { configuration, .. } => configuration,
        }
    }
}

/// Literal Maven notation at the source boundary; generation validates its scope/version.
#[derive(Debug, Clone)]
pub struct GradleDependency {
    pub configuration: String,
    pub group: String,
    pub artifact: String,
    pub version: String,
}
