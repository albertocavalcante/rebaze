//! Plan and serialize the supported Gradle Java subset without filesystem effects.
use crate::starlark::{
    BazelDep, ExportsFiles, File, Identifier, JavaLibrary, JavaTest, Load, MavenInstall, Module,
    Reference, UseExtension, UseRepo,
};
use crate::{GenerationError, GenerationResult};
use rebaze_gradle::{Dependency, GradleModule, GradleProject};
use std::collections::{BTreeMap, BTreeSet, HashMap};

pub struct BazelFile {
    pub path: String,
    pub content: String,
}
fn invalid(message: impl Into<String>) -> GenerationError {
    GenerationError::Gradle(message.into())
}
fn is_java(module: &GradleModule) -> bool {
    module
        .plugins
        .iter()
        .any(|p| matches!(p.id.as_str(), "java" | "java-library"))
}
fn directory(path: &str) -> String {
    path.trim_start_matches(':').replace(':', "/")
}

#[derive(Default)]
struct Dependencies {
    compile: BTreeSet<String>,
    exports: BTreeSet<String>,
    runtime: BTreeSet<String>,
    tests: BTreeSet<String>,
    test_runtime: BTreeSet<String>,
}

pub fn generate(project: &GradleProject) -> GenerationResult<HashMap<String, String>> {
    let mut modules = BTreeMap::new();
    for module in &project.modules {
        for file in module
            .sources
            .iter()
            .chain(&module.resources)
            .chain(&module.test_sources)
            .chain(&module.test_resources)
        {
            if file.is_empty()
                || file.contains(['\\', ':'])
                || file.chars().any(char::is_control)
                || file
                    .split('/')
                    .any(|part| part.is_empty() || matches!(part, "." | ".."))
            {
                return Err(invalid(format!(
                    "source/resource path must be relative within its module: {file:?}"
                )));
            }
        }
        for class in &module.test_classes {
            let source = format!("src/test/java/{}.java", class.replace('.', "/"));
            if !module.test_sources.contains(&source) {
                return Err(invalid(format!(
                    "test class {class:?} has no matching conventional source"
                )));
            }
        }
        let path = rebaze_gradle::project_path(&module.path).map_err(|e| invalid(e.to_string()))?;
        if modules.insert(path, module).is_some() {
            return Err(invalid("duplicate Gradle project path"));
        }
        if module.plugins.iter().any(|p| {
            p.version.is_some() || !matches!(p.id.as_str(), "java" | "java-library" | "base")
        }) {
            return Err(invalid(
                "only unversioned built-in java/java-library/base plugins are supported",
            ));
        }
        if !is_java(module)
            && (!module.sources.is_empty()
                || !module.test_sources.is_empty()
                || !module.resources.is_empty()
                || !module.test_resources.is_empty()
                || !module.dependencies.is_empty())
        {
            return Err(invalid(format!(
                "project {:?} has Java inputs without a Java plugin",
                module.path
            )));
        }
        if !module.test_sources.is_empty() && module.test_classes.is_empty() {
            return Err(invalid(format!(
                "project {:?} has test sources but no supported JUnit 4 test classes",
                module.path
            )));
        }
    }
    if !modules.values().any(|m| is_java(m)) {
        return Err(invalid("no supported Java projects found"));
    }
    let mut artifacts = BTreeMap::new();
    let mut aliases = BTreeMap::new();
    let mut plans = BTreeMap::new();
    let mut edges: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (path, module) in &modules {
        let mut deps = Dependencies::default();
        for dependency in &module.dependencies {
            let scope = dependency.configuration();
            if !matches!(
                scope,
                "implementation" | "api" | "runtimeOnly" | "testImplementation" | "testRuntimeOnly"
            ) {
                return Err(invalid(format!(
                    "unsupported dependency configuration {scope:?}"
                )));
            }
            if scope == "api" && !module.plugins.iter().any(|p| p.id == "java-library") {
                return Err(invalid("api dependencies require java-library"));
            }
            let label = match dependency {
                Dependency::Project { path: target, .. } => {
                    if !target.starts_with(':') {
                        return Err(invalid(
                            "project dependencies require absolute colon-prefixed paths",
                        ));
                    }
                    let target =
                        rebaze_gradle::project_path(target).map_err(|e| invalid(e.to_string()))?;
                    if !modules.get(&target).is_some_and(|m| is_java(m)) {
                        return Err(invalid(format!(
                            "unknown/non-Java project dependency {target:?}"
                        )));
                    }
                    if !scope.starts_with("test") {
                        edges
                            .entry(path.clone())
                            .or_default()
                            .insert(target.clone());
                    }
                    format!("//{}:lib", directory(&target))
                }
                Dependency::Maven(dep) => {
                    if !module.maven_central {
                        return Err(invalid(
                            "Maven dependencies require an explicit mavenCentral() repository",
                        ));
                    }
                    if [&dep.group, &dep.artifact].iter().any(|s| {
                        s.is_empty()
                            || !s.bytes().all(|c| {
                                c.is_ascii_lowercase()
                                    || c.is_ascii_digit()
                                    || matches!(c, b'.' | b'_' | b'-')
                            })
                    }) || dep.version.is_empty()
                        || !dep.version.starts_with(|c: char| c.is_ascii_digit())
                        || !dep
                            .version
                            .bytes()
                            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-'))
                        || dep.version.ends_with("SNAPSHOT")
                    {
                        return Err(invalid(
                            "Maven coordinates require literal lower-case group/artifact and fixed release versions",
                        ));
                    }
                    let key = format!("{}:{}", dep.group, dep.artifact);
                    if artifacts
                        .insert(key.clone(), dep.version.clone())
                        .is_some_and(|v| v != dep.version)
                    {
                        return Err(invalid(format!("conflicting Maven versions for {key}")));
                    }
                    let alias = key.replace(['.', '-', ':'], "_");
                    if aliases
                        .insert(alias.clone(), key.clone())
                        .is_some_and(|old| old != key)
                    {
                        return Err(invalid("Maven artifact labels collide after normalization"));
                    }
                    format!("@rebaze_maven//:{alias}")
                }
            };
            match scope {
                "implementation" => {
                    deps.compile.insert(label);
                }
                "api" => {
                    deps.exports.insert(label.clone());
                    deps.compile.insert(label);
                }
                "runtimeOnly" => {
                    deps.runtime.insert(label);
                }
                "testImplementation" => {
                    deps.tests.insert(label);
                }
                "testRuntimeOnly" => {
                    deps.test_runtime.insert(label);
                }
                _ => unreachable!("validated scope"),
            }
        }
        if !module.test_classes.is_empty() && !module.dependencies.iter().any(|dep| matches!(dep,Dependency::Maven(d) if d.group=="junit" && d.artifact=="junit" && d.version.starts_with("4.") && matches!(d.configuration.as_str(),"testImplementation"|"implementation"|"api"))) { return Err(invalid("JUnit 4 tests require an explicit junit:junit:4.x compile/testImplementation dependency")); }
        plans.insert(path.clone(), deps);
    }
    let mut resolved = BTreeSet::new();
    loop {
        let before = resolved.len();
        for path in modules.keys() {
            if edges.get(path).is_none_or(|deps| deps.is_subset(&resolved)) {
                resolved.insert(path.clone());
            }
        }
        if resolved.len() == modules.len() {
            break;
        }
        if resolved.len() == before {
            return Err(invalid("cyclic production project dependencies"));
        }
    }
    let mut module_file = File::new();
    module_file.docstring(&format!(
        "Bazel module for {} - generated by rebaze.",
        project.name
    ))?;
    module_file.statement(&Module {
        name: project.name.replace('-', "_"),
        version: "0.1.0".into(),
    })?;
    module_file.statement(&BazelDep {
        name: "rules_java".into(),
        version: "9.9.0".into(),
    })?;
    if !artifacts.is_empty() {
        module_file.statement(&BazelDep {
            name: "rules_jvm_external".into(),
            version: "6.10".into(),
        })?;
        module_file.assign(
            &Identifier::new("maven")?,
            &UseExtension("@rules_jvm_external//:extensions.bzl", "maven"),
        )?;
        module_file.statement(&MavenInstall {
            name: "rebaze_maven".into(),
            artifacts: artifacts.iter().map(|(k, v)| format!("{k}:{v}")).collect(),
            repositories: vec!["https://repo.maven.apache.org/maven2".into()],
            lock_file: project
                .maven_lock
                .as_ref()
                .map(|_| "//:maven_install.json".into()),
            aar_import_bzl_label: "@@//:rebaze_java_support.bzl".into(),
        })?;
        module_file.statement(&UseRepo {
            extension: Reference::new("maven")?,
            repositories: vec!["rebaze_maven".into()],
        })?;
    }
    let mut files = HashMap::from([("MODULE.bazel".into(),module_file.render()?), (".bazelversion".into(),"9.0.0\n".into()),(".bazelrc".into(),"build --java_language_version=21\nbuild --tool_java_language_version=21\nbuild --java_runtime_version=remotejdk_21\nbuild --tool_java_runtime_version=remotejdk_21\n".into())]);
    if let Some(lock) = &project.maven_lock {
        files.insert("maven_install.json".into(), lock.clone());
    }
    if !artifacts.is_empty() {
        files.insert(
            "rebaze_java_support.bzl".into(),
            include_str!("templates/java_support.bzl").into(),
        );
    }
    for (path, module) in modules {
        let mut file = File::new();
        if path.is_empty() {
            file.statement(&ExportsFiles(vec!["MODULE.bazel".into()]))?;
        }
        if is_java(module) {
            file.load(Load {
                bzl: "@rules_java//java:defs.bzl".into(),
                items: ["java_library".into(), "java_test".into()].into(),
            })?;
            let deps = &plans[&path];
            file.statement(&JavaLibrary {
                name: "lib".into(),
                srcs: module.sources.clone(),
                resources: module.resources.clone(),
                visibility: vec!["//visibility:public".into()],
                deps: deps.compile.iter().cloned().collect(),
                exports: deps.exports.iter().cloned().collect(),
                runtime_deps: deps.runtime.iter().cloned().collect(),
            })?;
            let mut names = BTreeSet::new();
            for class in &module.test_classes {
                if class.is_empty()
                    || !class.split('.').all(|part| {
                        !part.is_empty()
                            && part.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
                            && part.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
                    })
                {
                    return Err(invalid(format!("invalid Java test class {class:?}")));
                }
                let name = format!("test_{}", class.replace('.', "_"));
                if !names.insert(name.clone()) {
                    return Err(invalid("Java test target names collide"));
                }
                file.statement(&JavaTest {
                    name,
                    srcs: module.test_sources.clone(),
                    resources: module.test_resources.clone(),
                    test_class: class.clone(),
                    deps: std::iter::once(":lib".into())
                        .chain(deps.compile.iter().cloned())
                        .chain(deps.tests.iter().cloned())
                        .collect(),
                    runtime_deps: deps.test_runtime.iter().cloned().collect(),
                })?;
            }
        }
        let file_path = if path.is_empty() {
            "BUILD.bazel".into()
        } else {
            format!("{}/BUILD.bazel", directory(&path))
        };
        files.insert(file_path, file.render()?);
    }
    if !files.contains_key("BUILD.bazel") {
        let mut file = File::new();
        file.statement(&ExportsFiles(vec!["MODULE.bazel".into()]))?;
        files.insert("BUILD.bazel".into(), file.render()?);
    }
    Ok(files)
}
