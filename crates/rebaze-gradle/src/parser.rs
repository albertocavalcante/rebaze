//! Literal Gradle build metadata. Arbitrary DSL evaluation is deliberately unsupported.
use crate::model::{Dependency, GradleDependency, GradleModule, GradlePlugin};
use crate::syntax::Cursor;
use anyhow::{Result, bail};

fn plugin(cursor: &mut Cursor) -> Result<GradlePlugin> {
    let kind = cursor.word()?;
    let id = match kind.as_str() {
        "id" | "kotlin" => {
            let parentheses = cursor.symbol('(');
            let id = cursor.literal()?;
            if parentheses {
                cursor.expect(')')?;
            }
            if kind == "kotlin" {
                format!("org.jetbrains.kotlin.{id}")
            } else {
                id
            }
        }
        "java" | "base" => kind,
        _ => bail!("unsupported plugin declaration {kind:?}"),
    };
    let version = if cursor.word_is("version") {
        Some(cursor.literal()?)
    } else {
        None
    };
    Ok(GradlePlugin { id, version })
}

fn dependency(cursor: &mut Cursor) -> Result<Dependency> {
    let configuration = cursor.word()?;
    if !matches!(
        configuration.as_str(),
        "implementation"
            | "api"
            | "compileOnly"
            | "runtimeOnly"
            | "testImplementation"
            | "testRuntimeOnly"
    ) {
        bail!("unsupported dependency configuration {configuration:?}");
    }
    let parentheses = cursor.symbol('(');
    let dependency = if cursor.word_is("project") {
        cursor.expect('(')?;
        let path = cursor.literal()?;
        cursor.expect(')')?;
        Dependency::Project {
            configuration,
            path,
        }
    } else {
        let coordinate = cursor.literal()?;
        let parts: Vec<_> = coordinate.split(':').collect();
        if !(2..=3).contains(&parts.len()) {
            bail!("unsupported Maven coordinate {coordinate:?}");
        }
        Dependency::Maven(GradleDependency {
            configuration,
            group: parts[0].into(),
            artifact: parts[1].into(),
            version: parts.get(2).copied().unwrap_or_default().into(),
        })
    };
    if parentheses {
        cursor.expect(')')?;
    }
    Ok(dependency)
}

pub fn parse_build_source(source: &str) -> Result<GradleModule> {
    let mut cursor = Cursor::new(source)?;
    let mut module = GradleModule::default();
    while !cursor.done() {
        if cursor.symbol(';') {
            continue;
        }
        let block = cursor.word()?;
        if !matches!(block.as_str(), "plugins" | "dependencies" | "repositories") {
            bail!("unsupported Gradle block {block:?}");
        }
        cursor.expect('{')?;
        while !cursor.symbol('}') {
            if cursor.symbol(';') {
                continue;
            }
            match block.as_str() {
                "plugins" => module.plugins.push(plugin(&mut cursor)?),
                "dependencies" => module.dependencies.push(dependency(&mut cursor)?),
                "repositories" => {
                    if !cursor.word_is("mavenCentral") {
                        bail!("only mavenCentral() repositories are supported");
                    }
                    cursor.expect('(')?;
                    cursor.expect(')')?;
                    module.maven_central = true;
                }
                _ => bail!(
                    "unsupported Gradle block {block:?}; native migration accepts literal Java metadata only"
                ),
            }
        }
    }
    Ok(module)
}

pub fn settings(source: &str) -> Result<(Option<String>, Vec<String>)> {
    let mut cursor = Cursor::new(source)?;
    let mut name = None;
    let mut includes = Vec::new();
    while !cursor.done() {
        if cursor.symbol(';') {
            continue;
        }
        match cursor.word()?.as_str() {
            "rootProject" => {
                cursor.expect('.')?;
                if !cursor.word_is("name") {
                    bail!("unsupported rootProject setting");
                }
                cursor.expect('=')?;
                name = Some(cursor.literal()?);
            }
            "include" => {
                let parentheses = cursor.symbol('(');
                loop {
                    includes.push(cursor.literal()?);
                    if !cursor.symbol(',') {
                        break;
                    }
                }
                if parentheses {
                    cursor.expect(')')?;
                }
            }
            other => bail!(
                "unsupported settings declaration {other:?}; custom project directories/build logic require Gradle evaluation"
            ),
        }
    }
    Ok((name, includes))
}
#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::syntax::{Token, lex};

    #[test]
    fn literal_subset_rejects_uninterpreted_build_and_settings_logic() {
        for source in [
            "if (false) { dependencies { implementation 'x:y:1' } }",
            "sourceSets {}",
            "test { useJUnitPlatform() }",
            "unknown {}",
            "dependencies { implementation libs.example }",
            "dependencies { implementation 'x:y:1' { exclude group: 'z' } }",
        ] {
            assert!(parse_build_source(source).is_err(), "{source}");
        }
        assert!(settings("if (false) { include('app') }").is_err());
        assert!(settings("project(':app').projectDir = file('../outside')").is_err());
    }
    fn parse_plugin_line(source: &str) -> Option<GradlePlugin> {
        plugin(&mut Cursor::new(source).ok()?).ok()
    }
    fn parse_dependency_line(source: &str) -> Option<GradleDependency> {
        match dependency(&mut Cursor::new(source).ok()?).ok()? {
            Dependency::Maven(dep) => Some(dep),
            Dependency::Project { .. } => None,
        }
    }
    fn parse_plugins(source: &str, _kts: bool, output: &mut Vec<GradlePlugin>) {
        output.extend(parse_build_source(source).unwrap().plugins);
    }
    fn parse_dependencies(source: &str, _kts: bool, output: &mut Vec<GradleDependency>) {
        output.extend(
            parse_build_source(source)
                .unwrap()
                .dependencies
                .into_iter()
                .filter_map(|dep| match dep {
                    Dependency::Maven(dep) => Some(dep),
                    Dependency::Project { .. } => None,
                }),
        );
    }
    fn extract_quoted_string(source: &str) -> Option<String> {
        lex(source).ok()?.into_iter().find_map(|token| match token {
            Token::Literal(value) => Some(value),
            _ => None,
        })
    }

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
