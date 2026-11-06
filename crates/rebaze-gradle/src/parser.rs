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

    let (prefix, kotlin) = if let Some(rest) = line.strip_prefix("id") {
        (rest, false)
    } else {
        (line.strip_prefix("kotlin")?, true)
    };
    if !prefix.starts_with('(') && !prefix.starts_with(char::is_whitespace) {
        return None;
    }
    let (id, rest) = quoted_value(prefix)?;
    let id = if kotlin {
        format!("org.jetbrains.kotlin.{id}")
    } else {
        id.to_owned()
    };
    let version = rest
        .trim_start_matches(|c: char| c == ')' || c.is_whitespace())
        .strip_prefix("version")
        .and_then(extract_quoted_string);
    Some(GradlePlugin { id, version })
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
            let rest = line.strip_prefix(config)?;
            if !rest.starts_with('(') && !rest.starts_with(char::is_whitespace) {
                continue;
            }
            let rest = rest.trim();

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

fn extract_quoted_string(value: &str) -> Option<String> {
    quoted_value(value).map(|(quoted, _)| quoted.to_owned())
}

/// Return the first literal and the suffix after its matching closing quote.
fn quoted_value(value: &str) -> Option<(&str, &str)> {
    let (start, quote) = value
        .char_indices()
        .find(|(_, c)| matches!(c, '\'' | '"'))?;
    let suffix = &value[start + quote.len_utf8()..];
    let end = suffix.find(quote)?;
    Some((&suffix[..end], &suffix[end + quote.len_utf8()..]))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn plugin_ids_containing_version_do_not_confuse_the_version_clause() {
        let plugin = parse_plugin_line(r#"id("com.version.plugin") version "1.2.3""#).unwrap();
        assert_eq!(plugin.id, "com.version.plugin");
        assert_eq!(plugin.version.as_deref(), Some("1.2.3"));
        assert!(parse_plugin_line(r#"identity("not-a-plugin")"#).is_none());
    }

    #[test]
    fn configuration_prefixes_are_not_dependency_declarations() {
        assert!(parse_dependency_line(r#"implementationExtra("org.example:lib:1.0")"#).is_none());
    }

    // ============================================================================
    // Plugin Parsing Tests
    // ============================================================================

    #[test]
    fn test_parse_plugin_line_id_with_quotes() {
        let line = r#"id("com.android.application")"#;
        let plugin = parse_plugin_line(line);

        assert!(plugin.is_some());
        let plugin = plugin.unwrap();
        assert_eq!(plugin.id, "com.android.application");
        assert!(plugin.version.is_none());
    }

    #[test]
    fn test_parse_plugin_line_id_with_version() {
        let line = r#"id("com.example.plugin") version "1.2.3""#;
        let plugin = parse_plugin_line(line);

        assert!(plugin.is_some());
        let plugin = plugin.unwrap();
        assert_eq!(plugin.id, "com.example.plugin");
        assert_eq!(plugin.version, Some("1.2.3".to_string()));
    }

    #[test]
    fn test_parse_plugin_line_kotlin() {
        let line = r#"kotlin("jvm")"#;
        let plugin = parse_plugin_line(line);

        assert!(plugin.is_some());
        let plugin = plugin.unwrap();
        assert_eq!(plugin.id, "org.jetbrains.kotlin.jvm");
        assert!(plugin.version.is_none());
    }

    #[test]
    fn test_parse_plugin_line_kotlin_with_version() {
        let line = r#"kotlin("jvm") version "1.9.0""#;
        let plugin = parse_plugin_line(line);

        assert!(plugin.is_some());
        let plugin = plugin.unwrap();
        assert_eq!(plugin.id, "org.jetbrains.kotlin.jvm");
        assert_eq!(plugin.version, Some("1.9.0".to_string()));
    }

    #[test]
    fn test_parse_plugin_line_invalid() {
        let line = "apply plugin: 'java'";
        let plugin = parse_plugin_line(line);
        assert!(plugin.is_none());
    }

    #[test]
    fn test_parse_plugins_block() {
        let content = r#"
plugins {
    id("com.android.application")
    kotlin("jvm") version "1.9.0"
}
"#;
        let mut plugins = Vec::new();
        parse_plugins(content, true, &mut plugins);

        assert_eq!(plugins.len(), 2);
        assert_eq!(plugins[0].id, "com.android.application");
        assert_eq!(plugins[1].id, "org.jetbrains.kotlin.jvm");
    }

    // ============================================================================
    // Dependency Parsing Tests
    // ============================================================================

    #[test]
    fn test_parse_dependency_line_implementation() {
        let line = r#"implementation("com.google.guava:guava:32.1.3-jre")"#;
        let dep = parse_dependency_line(line);

        assert!(dep.is_some());
        let dep = dep.unwrap();
        assert_eq!(dep.configuration, "implementation");
        assert_eq!(dep.group, "com.google.guava");
        assert_eq!(dep.artifact, "guava");
        assert_eq!(dep.version, "32.1.3-jre");
    }

    #[test]
    fn test_parse_dependency_line_test_implementation() {
        let line = r#"testImplementation("org.junit.jupiter:junit-jupiter:5.10.0")"#;
        let dep = parse_dependency_line(line);

        assert!(dep.is_some());
        let dep = dep.unwrap();
        assert_eq!(dep.configuration, "testImplementation");
        assert_eq!(dep.group, "org.junit.jupiter");
        assert_eq!(dep.artifact, "junit-jupiter");
    }

    #[test]
    fn test_parse_dependency_line_api() {
        let line = r#"api("io.grpc:grpc-core:1.60.0")"#;
        let dep = parse_dependency_line(line);

        assert!(dep.is_some());
        let dep = dep.unwrap();
        assert_eq!(dep.configuration, "api");
    }

    #[test]
    fn test_parse_dependency_line_no_version() {
        let line = r#"implementation("com.example:library")"#;
        let dep = parse_dependency_line(line);

        assert!(dep.is_some());
        let dep = dep.unwrap();
        assert_eq!(dep.group, "com.example");
        assert_eq!(dep.artifact, "library");
        assert_eq!(dep.version, "");
    }

    #[test]
    fn test_parse_dependency_line_invalid() {
        let line = "// This is a comment";
        let dep = parse_dependency_line(line);
        assert!(dep.is_none());
    }

    #[test]
    fn test_parse_dependencies_block() {
        let content = r#"
dependencies {
    implementation("com.google.guava:guava:32.1.3-jre")
    testImplementation("org.junit.jupiter:junit-jupiter:5.10.0")
}
"#;
        let mut deps = Vec::new();
        parse_dependencies(content, true, &mut deps);

        assert_eq!(deps.len(), 2);
        assert_eq!(deps[0].configuration, "implementation");
        assert_eq!(deps[1].configuration, "testImplementation");
    }

    // ============================================================================
    // extract_quoted_string Tests
    // ============================================================================

    #[test]
    fn test_extract_quoted_string_double_quotes() {
        let s = r#"id("hello.world")"#;
        let result = extract_quoted_string(s);
        assert_eq!(result, Some("hello.world".to_string()));
    }

    #[test]
    fn test_extract_quoted_string_single_quotes() {
        let s = "id('hello.world')";
        let result = extract_quoted_string(s);
        assert_eq!(result, Some("hello.world".to_string()));
    }

    #[test]
    fn test_extract_quoted_string_no_quotes() {
        let s = "no quotes here";
        let result = extract_quoted_string(s);
        assert!(result.is_none());
    }
}
