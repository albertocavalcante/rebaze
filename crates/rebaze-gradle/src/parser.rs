//! Gradle build file parsing.

use anyhow::Result;
use std::path::{Path, PathBuf};

/// Represents a parsed Gradle project.
#[derive(Debug, Clone)]
pub struct GradleProject {
    pub name: String,
    pub path: PathBuf,
    pub subprojects: Vec<String>,
    pub plugins: Vec<GradlePlugin>,
    pub dependencies: Vec<GradleDependency>,
}

#[derive(Debug, Clone)]
pub struct GradlePlugin {
    pub id: String,
    pub version: Option<String>,
}

#[derive(Debug, Clone)]
pub struct GradleDependency {
    pub configuration: String, // implementation, api, testImplementation, etc.
    pub group: String,
    pub artifact: String,
    pub version: String,
}

/// Parse a build.gradle or build.gradle.kts file.
pub fn parse_build_file(path: &Path, project: &mut GradleProject) -> Result<()> {
    let content = std::fs::read_to_string(path)?;
    let is_kts = path.extension().is_some_and(|e| e == "kts");

    parse_plugins(&content, is_kts, &mut project.plugins);
    parse_dependencies(&content, is_kts, &mut project.dependencies);

    Ok(())
}

fn parse_plugins(content: &str, _is_kts: bool, plugins: &mut Vec<GradlePlugin>) {
    // Simple line-by-line parsing for plugins
    let mut in_plugins_block = false;

    for line in content.lines() {
        let line = line.trim();

        if line.starts_with("plugins") && line.contains('{') {
            in_plugins_block = true;
            continue;
        }

        if in_plugins_block {
            if line == "}" {
                in_plugins_block = false;
                continue;
            }

            // Parse: id("com.example.plugin") version "1.0"
            // or: kotlin("jvm") version "1.9.0"
            if let Some(plugin) = parse_plugin_line(line) {
                plugins.push(plugin);
            }
        }
    }
}

fn parse_plugin_line(line: &str) -> Option<GradlePlugin> {
    let line = line.trim();

    // Handle: id("plugin.id") or id "plugin.id"
    if line.starts_with("id") {
        let id = extract_quoted_string(line)?;
        let version = if line.contains("version") {
            line.split("version").nth(1).and_then(extract_quoted_string)
        } else {
            None
        };

        return Some(GradlePlugin { id, version });
    }

    // Handle: kotlin("jvm")
    if line.starts_with("kotlin") {
        let variant = extract_quoted_string(line)?;
        let id = format!("org.jetbrains.kotlin.{variant}");
        let version = if line.contains("version") {
            line.split("version").nth(1).and_then(extract_quoted_string)
        } else {
            None
        };

        return Some(GradlePlugin { id, version });
    }

    None
}

fn parse_dependencies(content: &str, _is_kts: bool, deps: &mut Vec<GradleDependency>) {
    let mut in_deps_block = false;

    for line in content.lines() {
        let line = line.trim();

        if line.starts_with("dependencies") && line.contains('{') {
            in_deps_block = true;
            continue;
        }

        if in_deps_block {
            if line == "}" {
                in_deps_block = false;
                continue;
            }

            if let Some(dep) = parse_dependency_line(line) {
                deps.push(dep);
            }
        }
    }
}

fn parse_dependency_line(line: &str) -> Option<GradleDependency> {
    let configs = [
        "implementation",
        "api",
        "compileOnly",
        "runtimeOnly",
        "testImplementation",
        "testRuntimeOnly",
    ];

    for config in configs {
        if line.starts_with(config) {
            let rest = line.strip_prefix(config)?.trim();

            // Parse "group:artifact:version" format
            let coords = extract_quoted_string(rest)?;
            let parts: Vec<&str> = coords.split(':').collect();

            if parts.len() >= 2 {
                return Some(GradleDependency {
                    configuration: config.to_string(),
                    group: parts[0].to_string(),
                    artifact: parts[1].to_string(),
                    version: parts.get(2).copied().unwrap_or("").to_string(),
                });
            }
        }
    }

    None
}

fn extract_quoted_string(s: &str) -> Option<String> {
    // Find content between quotes (single or double) or parentheses with quotes
    let s = s.trim();

    for (open, close) in [("\"", "\""), ("'", "'"), ("(\"", "\")"), ("('", "')")] {
        if let Some(start) = s.find(open) {
            let after_open = start + open.len();
            if let Some(end) = s[after_open..].find(close) {
                return Some(s[after_open..after_open + end].to_string());
            }
        }
    }

    None
}
