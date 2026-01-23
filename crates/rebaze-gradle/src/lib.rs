//! Gradle build file parser for rebaze.

use anyhow::{Context, Result};
use std::path::Path;
use walkdir::WalkDir;

mod parser;

pub use parser::GradleProject;

/// Parse a Gradle project at the given path.
pub fn parse(path: &Path) -> Result<GradleProject> {
    tracing::debug!("Parsing Gradle project at {}", path.display());

    let settings_file = find_settings_file(path)?;
    let root_build = find_build_file(path);

    let mut project = GradleProject {
        name: extract_project_name(&settings_file)?,
        path: path.to_path_buf(),
        subprojects: Vec::new(),
        plugins: Vec::new(),
        dependencies: Vec::new(),
    };

    // Parse root build file if exists
    if let Some(build_file) = root_build {
        parser::parse_build_file(&build_file, &mut project)?;
    }

    // Find and parse subprojects
    for entry in WalkDir::new(path)
        .max_depth(3)
        .into_iter()
        .filter_map(std::result::Result::ok)
    {
        let entry_path = entry.path();
        if entry_path == path {
            continue;
        }

        if entry_path.join("build.gradle").exists() || entry_path.join("build.gradle.kts").exists()
        {
            let subproject_name = entry_path
                .strip_prefix(path)
                .unwrap_or(entry_path)
                .to_string_lossy()
                .replace('/', ":");

            project.subprojects.push(subproject_name);
        }
    }

    tracing::info!(
        "Parsed Gradle project '{}' with {} subprojects",
        project.name,
        project.subprojects.len()
    );

    Ok(project)
}

fn find_settings_file(path: &Path) -> Result<std::path::PathBuf> {
    let kts = path.join("settings.gradle.kts");
    if kts.exists() {
        return Ok(kts);
    }

    let groovy = path.join("settings.gradle");
    if groovy.exists() {
        return Ok(groovy);
    }

    anyhow::bail!(
        "No settings.gradle or settings.gradle.kts found at {}",
        path.display()
    )
}

fn find_build_file(path: &Path) -> Option<std::path::PathBuf> {
    let kts = path.join("build.gradle.kts");
    if kts.exists() {
        return Some(kts);
    }

    let groovy = path.join("build.gradle");
    if groovy.exists() {
        return Some(groovy);
    }

    None
}

fn extract_project_name(settings_file: &Path) -> Result<String> {
    let content = std::fs::read_to_string(settings_file).context("Failed to read settings file")?;

    // Simple regex-free parsing for rootProject.name
    for line in content.lines() {
        let line = line.trim();
        if line.starts_with("rootProject.name") {
            if let Some(name) = line
                .split('=')
                .nth(1)
                .map(|s| s.trim().trim_matches(|c| c == '"' || c == '\''))
            {
                return Ok(name.to_string());
            }
        }
    }

    // Fallback to directory name
    Ok(settings_file
        .parent()
        .and_then(|p| p.file_name())
        .map_or_else(
            || "unknown".to_string(),
            |n| n.to_string_lossy().into_owned(),
        ))
}
