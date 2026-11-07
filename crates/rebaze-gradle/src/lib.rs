//! Source extraction for a conservative literal Gradle Java subset.

use anyhow::{Context, Result, bail};
use std::collections::BTreeSet;
use std::path::Path;
use walkdir::WalkDir;

mod java;
mod model;
mod parser;
mod syntax;
pub use model::*;

/// Source/IO diagnostics retain their context behind a library error type.
#[derive(Debug, thiserror::Error)]
#[error("invalid Gradle Java input: {0:#}")]
pub struct GradleError(#[source] anyhow::Error);

pub fn parse_build_source(source: &str) -> std::result::Result<GradleModule, GradleError> {
    parser::parse_build_source(source).map_err(GradleError)
}

/// Normalize a literal Gradle project path without admitting filesystem traversal.
pub fn project_path(value: &str) -> std::result::Result<String, GradleError> {
    normalize_project_path(value).map_err(GradleError)
}

fn normalize_project_path(value: &str) -> Result<String> {
    if value.is_empty() || value == ":" {
        return Ok(String::new());
    }
    let value = value.strip_prefix(':').unwrap_or(value);
    if value.split(':').any(|part| {
        part.is_empty()
            || !part
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-'))
    }) {
        bail!("unsupported Gradle project path {value:?}");
    }
    Ok(format!(":{value}"))
}

pub fn parse(path: &Path) -> std::result::Result<GradleProject, GradleError> {
    read_project(path).map_err(GradleError)
}

fn read_project(path: &Path) -> Result<GradleProject> {
    let root = path.canonicalize().context("resolve Gradle project root")?;
    let settings = select_file(&root, "settings.gradle", "settings.gradle.kts")?
        .context("No settings.gradle or settings.gradle.kts found")?;
    let (name, includes) = parser::settings(&std::fs::read_to_string(&settings)?)
        .with_context(|| format!("unsupported settings in {}", settings.display()))?;
    let name = name.unwrap_or_else(|| {
        root.file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned()
    });
    let mut paths = BTreeSet::from([String::new()]);
    let mut explicit = BTreeSet::new();
    for include in includes {
        let include = project_path(&include)?;
        if include.is_empty() || !explicit.insert(include.clone()) {
            bail!("duplicate/empty included project {include:?}");
        }
        let mut ancestor = String::new();
        for part in include.trim_start_matches(':').split(':') {
            ancestor.push(':');
            ancestor.push_str(part);
            paths.insert(ancestor.clone());
        }
    }
    let mut modules = Vec::new();
    for project in paths {
        let directory = root.join(project.trim_start_matches(':').replace(':', "/"));
        if !directory.canonicalize()?.starts_with(&root) {
            bail!("project directory escapes root: {project}");
        }
        let mut module =
            if let Some(build) = select_file(&directory, "build.gradle", "build.gradle.kts")? {
                parse_build_source(&std::fs::read_to_string(&build)?)
                    .with_context(|| format!("unsupported build metadata in {}", build.display()))?
            } else {
                GradleModule::default()
            };
        module.path = project;
        module.sources = files(&directory, "src/main/java", true)?;
        module.resources = files(&directory, "src/main/resources", false)?;
        module.test_sources = files(&directory, "src/test/java", true)?;
        module.test_resources = files(&directory, "src/test/resources", false)?;
        for source in &module.test_sources {
            if let Some(class) =
                java::test_class(source, &std::fs::read_to_string(directory.join(source))?)?
            {
                module.test_classes.push(class);
            }
        }
        modules.push(module);
    }
    let lock = root.join("maven_install.json");
    let maven_lock = match std::fs::symlink_metadata(&lock) {
        Ok(metadata) if metadata.is_file() => Some(std::fs::read_to_string(&lock)?),
        Ok(_) => bail!("Maven lock must be a regular project file"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    Ok(GradleProject {
        name,
        path: root,
        modules,
        maven_lock,
    })
}

fn select_file(root: &Path, groovy: &str, kotlin: &str) -> Result<Option<std::path::PathBuf>> {
    let groovy = root.join(groovy);
    let kotlin = root.join(kotlin);
    if groovy.is_symlink() || kotlin.is_symlink() {
        bail!("Gradle build/settings symlinks require explicit mapping");
    }
    match (groovy.is_file(), kotlin.is_file()) {
        (true, true) => bail!(
            "ambiguous Gradle files: {} and {}",
            groovy.display(),
            kotlin.display()
        ),
        (true, false) => Ok(Some(groovy)),
        (false, true) => Ok(Some(kotlin)),
        (false, false) => Ok(None),
    }
}

fn files(root: &Path, directory: &str, java: bool) -> Result<Vec<String>> {
    let mut ancestor = root.to_path_buf();
    for part in Path::new(directory).components() {
        ancestor.push(part);
        if ancestor.is_symlink() {
            bail!(
                "source symlinks need explicit mapping: {}",
                ancestor.display()
            );
        }
    }
    let directory = root.join(directory);
    if !directory.exists() {
        return Ok(Vec::new());
    }
    let mut files = Vec::new();
    for entry in WalkDir::new(&directory) {
        let entry = entry?;
        if entry.file_type().is_symlink() {
            bail!(
                "source symlinks need explicit mapping: {}",
                entry.path().display()
            );
        }
        if entry.file_type().is_file()
            && (!java || entry.path().extension().is_some_and(|ext| ext == "java"))
        {
            files.push(
                entry
                    .path()
                    .strip_prefix(root)?
                    .to_str()
                    .context("source paths must be UTF-8")?
                    .replace('\\', "/"),
            );
        }
    }
    files.sort();
    Ok(files)
}
