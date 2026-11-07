//! Pure validation and resolution for managed pkg-config dependencies.

use std::collections::{BTreeMap, BTreeSet};

use rebaze_cmake::{CMakeProject, PkgConfigModule};
use serde::ser::SerializeMap;
use serde::{Serialize, Serializer};
use thiserror::Error;

use crate::config::{KnownPackageInfo, MigrationConfig, SourceBuildSystem};
use crate::discovery::target_name;

/// Only implemented strategies can enter a generation plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DependencyStrategy {
    System,
    Source,
    Bcr,
}

impl DependencyStrategy {
    fn parse(value: &str) -> Result<Self> {
        match value {
            "system" => Ok(Self::System),
            "source" => Ok(Self::Source),
            "bcr" => Ok(Self::Bcr),
            _ => Err(PlanError(format!(
                "unsupported dependency strategy {value:?}; use system, source, or bcr"
            ))),
        }
    }
}

/// Invalid intent is rejected before discovery or rendering.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("invalid dependency plan: {0}")]
pub struct PlanError(pub(crate) String);

type Result<T> = std::result::Result<T, PlanError>;

#[derive(Debug)]
pub struct PlannedAlias {
    pub strategy: DependencyStrategy,
    pub packages: Vec<String>,
    pub targets: Vec<String>,
}

impl Serialize for PlannedAlias {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(3))?;
        map.serialize_entry("strategy", &self.strategy)?;
        map.serialize_entry("packages", &self.packages)?;
        map.serialize_entry("targets", &self.targets)?;
        map.end()
    }
}

#[derive(Debug)]
pub enum SourceOptions {
    Meson(BTreeMap<String, String>),
    Cmake(Vec<String>),
    Autotools(Vec<String>),
}

#[derive(Debug)]
pub struct PlannedSource {
    pub info: KnownPackageInfo,
    pub options: SourceOptions,
    pub deps: Vec<String>,
}

/// Immutable output intent, independent of installed tools or host paths.
#[derive(Debug, Default)]
pub struct DependencyPlan {
    pub aliases: BTreeMap<String, PlannedAlias>,
    pub systems: Vec<PkgConfigModule>,
    pub sources: BTreeMap<String, PlannedSource>,
    pub modules: BTreeMap<String, String>,
    // CMake extraction represents ${PREFIX_LIBRARIES} as synthetic -l flags.
    managed_links: BTreeMap<String, String>,
}

impl DependencyPlan {
    pub fn new(project: &CMakeProject, config: &MigrationConfig) -> Result<Self> {
        let default = DependencyStrategy::parse(&config.strategy.default)?;
        let fallback =
            DependencyStrategy::parse(config.strategy.bcr_fallback.as_deref().unwrap_or("system"))?;
        if fallback == DependencyStrategy::Bcr {
            return Err(PlanError("bcr_fallback must be system or source".into()));
        }
        let overrides = config
            .strategy
            .overrides
            .iter()
            .map(|(key, value)| Ok((key.clone(), DependencyStrategy::parse(value)?)))
            .collect::<Result<BTreeMap<_, _>>>()?;
        let mut builder = Builder {
            config,
            fallback,
            overrides,
            plan: Self::default(),
            visiting: BTreeSet::new(),
        };
        for (name, version) in crate::cmake_generator::base_dependencies(project, config) {
            builder.module(&name, &version)?;
        }
        let mut requests = BTreeMap::<String, PkgConfigModule>::new();
        for request in &project.pkg_config_modules {
            let alias = target_name(&request.prefix);
            validate_name(&alias)?;
            builder.plan.managed_links.insert(
                format!("PkgConfig::{}", request.prefix),
                format!("//third_party:{alias}"),
            );
            if let Some(previous) = requests.get(&alias) {
                if previous.packages != request.packages {
                    return Err(PlanError(format!(
                        "conflicting requests normalize to alias {alias:?}"
                    )));
                }
                continue;
            }
            if requests.len() >= 1024 || request.packages.is_empty() || request.packages.len() > 256
            {
                return Err(PlanError(
                    "expected at most 1024 aliases and 1..=256 packages per alias".into(),
                ));
            }
            requests.insert(alias.clone(), request.clone());
            let package_names = request
                .packages
                .iter()
                .map(|p| package_name(p))
                .collect::<Result<Vec<_>>>()?;
            let strategy = builder.alias_strategy(&alias, &package_names, default)?;
            let targets = if strategy == DependencyStrategy::System {
                vec![builder.system(request.clone())?]
            } else {
                let mut targets = BTreeSet::new();
                for (expression, package) in request.packages.iter().zip(&package_names) {
                    // The prefix is an identity hint only for a single-package alias.
                    let hint = (request.packages.len() == 1).then_some(alias.as_str());
                    targets.insert(builder.resolve(package, expression, hint, strategy)?);
                }
                targets.into_iter().collect()
            };
            for package in package_names {
                let label = format!("//third_party:{alias}");
                let key = format!("-l{package}");
                if let Some(previous) = builder
                    .plan
                    .managed_links
                    .insert(key.clone(), label.clone())
                    && previous != label
                {
                    return Err(PlanError(format!(
                        "pkg-config link {key:?} belongs to multiple aliases; use one alias for this request"
                    )));
                }
                builder
                    .plan
                    .managed_links
                    .insert(format!("PkgConfig::{}", request.prefix), label);
            }
            builder.plan.aliases.insert(
                alias,
                PlannedAlias {
                    strategy,
                    packages: request.packages.clone(),
                    targets,
                },
            );
        }
        for name in builder.plan.sources.keys() {
            if builder.plan.aliases.contains_key(&format!("{name}_source")) {
                return Err(PlanError(format!(
                    "source target {name}_source collides with a pkg-config alias"
                )));
            }
        }
        let mut repositories: BTreeSet<_> = builder.plan.modules.keys().cloned().collect();
        for repository in builder
            .plan
            .sources
            .keys()
            .map(|name| format!("{name}_src"))
            .chain(
                builder
                    .plan
                    .systems
                    .iter()
                    .map(|request| format!("system_{}", target_name(&request.prefix))),
            )
        {
            if !repositories.insert(repository.clone()) {
                return Err(PlanError(format!(
                    "conflicting apparent repository name {repository:?}"
                )));
            }
        }
        builder
            .plan
            .systems
            .sort_by_key(|request| target_name(&request.prefix));
        Ok(builder.plan)
    }

    pub fn managed_link(&self, link: &str) -> Option<String> {
        self.managed_links.get(link).cloned()
    }
}

struct Builder<'a> {
    config: &'a MigrationConfig,
    fallback: DependencyStrategy,
    overrides: BTreeMap<String, DependencyStrategy>,
    plan: DependencyPlan,
    visiting: BTreeSet<String>,
}

impl Builder<'_> {
    fn alias_strategy(
        &self,
        alias: &str,
        packages: &[String],
        default: DependencyStrategy,
    ) -> Result<DependencyStrategy> {
        if let Some(strategy) = self.overrides.get(alias) {
            return Ok(*strategy);
        }
        let mut selected = None;
        for name in packages {
            let normalized = name
                .strip_suffix("-2.0")
                .or_else(|| name.strip_suffix("-1.0"))
                .unwrap_or(name);
            let strategies = self
                .overrides
                .get(name)
                .or_else(|| self.overrides.get(normalized))
                .copied()
                .map_or_else(
                    || {
                        self.config
                            .mappings
                            .known_packages
                            .iter()
                            .filter(|(_, info)| {
                                info.pkg_config_packages
                                    .iter()
                                    .any(|package| package == name)
                            })
                            .filter_map(|(key, _)| self.overrides.get(key).copied())
                            .collect::<Vec<_>>()
                    },
                    |strategy| vec![strategy],
                );
            for strategy in strategies {
                if selected.is_some_and(|previous| previous != strategy) {
                    return Err(PlanError(format!(
                        "conflicting package strategies for alias {alias:?}; set a normalized prefix override"
                    )));
                }
                selected = Some(strategy);
            }
        }
        Ok(selected.unwrap_or(default))
    }

    fn source_identity(&self, package: &str, hint: Option<&str>) -> Result<Option<String>> {
        // Package identity takes precedence over an arbitrary CMake prefix.
        for name in [Some(package), hint].into_iter().flatten() {
            let normalized = name
                .strip_suffix("-2.0")
                .or_else(|| name.strip_suffix("-1.0"))
                .unwrap_or(name);
            for key in [name, normalized] {
                if self.config.mappings.known_packages.contains_key(key) {
                    return Ok(Some(key.into()));
                }
            }
            let matches = self
                .config
                .mappings
                .known_packages
                .iter()
                .filter(|(_, info)| {
                    info.pkg_config_packages
                        .iter()
                        .any(|request| request == name)
                })
                .map(|(key, _)| key)
                .collect::<Vec<_>>();
            match matches.as_slice() {
                [] => {}
                [key] => return Ok(Some((*key).clone())),
                _ => {
                    return Err(PlanError(format!(
                        "ambiguous source identity {name:?}: {matches:?}"
                    )));
                }
            }
        }
        Ok(None)
    }

    fn module(&mut self, name: &str, version: &str) -> Result<()> {
        if !name
            .as_bytes()
            .last()
            .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
            || !name.as_bytes().first().is_some_and(u8::is_ascii_lowercase)
            || !name.bytes().all(|byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"._-".contains(&byte)
            })
        {
            return Err(PlanError(format!("invalid Bazel module name {name:?}")));
        }
        if version.is_empty() || version.chars().any(char::is_control) {
            return Err(PlanError(format!("invalid version for module {name:?}")));
        }
        if let Some(previous) = self.plan.modules.insert(name.into(), version.into())
            && previous != version
        {
            return Err(PlanError(format!(
                "conflicting versions for module {name:?}: {previous:?} and {version:?}"
            )));
        }
        Ok(())
    }

    fn system(&mut self, request: PkgConfigModule) -> Result<String> {
        let name = target_name(&request.prefix);
        validate_name(&name)?;
        if let Some(previous) = self
            .plan
            .systems
            .iter_mut()
            .find(|previous| target_name(&previous.prefix) == name)
        {
            if previous.packages != request.packages {
                let names = |packages: &[String]| {
                    packages
                        .iter()
                        .map(|expression| package_name(expression))
                        .collect::<Result<BTreeSet<_>>>()
                };
                if names(&previous.packages)? != names(&request.packages)? {
                    return Err(PlanError(format!(
                        "conflicting system requests for {name:?}"
                    )));
                }
                // Retain the intersection of predicates; an unversioned duplicate adds no restriction.
                let combined: BTreeSet<_> = previous
                    .packages
                    .iter()
                    .chain(&request.packages)
                    .cloned()
                    .collect();
                previous.packages = combined
                    .iter()
                    .filter(|expression| {
                        expression.contains(['<', '>', '='])
                            || !combined.iter().any(|other| {
                                other.contains(['<', '>', '='])
                                    && package_name(other).ok() == package_name(expression).ok()
                            })
                    })
                    .cloned()
                    .collect();
            }
        } else {
            self.plan.systems.push(request);
        }
        Ok(format!("@system_{name}//:{name}"))
    }

    fn resolve(
        &mut self,
        package: &str,
        expression: &str,
        hint: Option<&str>,
        strategy: DependencyStrategy,
    ) -> Result<String> {
        match strategy {
            DependencyStrategy::System => self.system(PkgConfigModule {
                prefix: target_name(package),
                packages: vec![expression.into()],
                required: true,
            }),
            DependencyStrategy::Source => self.source(package, expression, hint),
            DependencyStrategy::Bcr => {
                if self
                    .config
                    .strategy
                    .bcr
                    .skip_packages
                    .iter()
                    .any(|skip| skip == package || Some(skip.as_str()) == hint)
                {
                    return self.resolve(package, expression, hint, self.fallback);
                }
                // Resolve separately so different targets in the same module survive import deduplication.
                let resolution = rebaze_bcr::resolve_cmake_deps(&[package.into()]);
                let Some(dep) = resolution.resolved.into_iter().next() else {
                    return self.resolve(package, expression, hint, self.fallback);
                };
                let module = self
                    .config
                    .strategy
                    .bcr
                    .module_overrides
                    .get(&dep.module)
                    .cloned()
                    .unwrap_or_else(|| dep.module.clone());
                let version = self
                    .config
                    .strategy
                    .bcr
                    .version_pins
                    .get(&module)
                    .cloned()
                    .unwrap_or(dep.version);
                validate_version(expression, package, &version)?;
                self.module(&module, &version)?;
                let old_target = dep.target.unwrap_or_else(|| format!("@{}", dep.module));
                let old_repo = format!("@{}", dep.module);
                let suffix = old_target
                    .strip_prefix(&old_repo)
                    .filter(|suffix| suffix.is_empty() || suffix.starts_with("//"))
                    .ok_or_else(|| {
                        PlanError(format!(
                            "BCR target {old_target:?} does not belong to module {:?}",
                            dep.module
                        ))
                    })?;
                // An override must preserve the package/target layout; users verify this contract.
                if suffix.is_empty() && module != dep.module {
                    Ok(format!("@{module}//:{}", dep.module))
                } else {
                    Ok(format!("@{module}{suffix}"))
                }
            }
        }
    }

    fn source(&mut self, package: &str, expression: &str, hint: Option<&str>) -> Result<String> {
        let name = self
            .source_identity(package, hint)?
            .ok_or_else(|| PlanError(format!("no source metadata for {package:?}")))?;
        validate_name(&name)?;
        let info = self.config.mappings.known_packages[&name].clone();
        validate_version(expression, package, &info.version)?;
        if self.visiting.contains(&name) {
            return Err(PlanError(format!("source dependency cycle at {name:?}")));
        }
        if !self.plan.sources.contains_key(&name) {
            if self.visiting.len() >= 64 || self.plan.sources.len() + self.visiting.len() >= 1024 {
                return Err(PlanError(
                    "source graph exceeds 64 levels or 1024 packages".into(),
                ));
            }
            let options = validate_source(&name, &info)?;
            self.visiting.insert(name.clone());
            let mut deps = BTreeSet::new();
            for dependency in info
                .deps
                .iter()
                .cloned()
                .chain(self.config.get_transitive_deps(&name))
            {
                let dependency = dependency.trim().to_owned();
                let package = package_name(&dependency)?;
                let strategy = self.alias_strategy(
                    &target_name(&package),
                    std::slice::from_ref(&package),
                    DependencyStrategy::Source,
                )?;
                let target = if strategy == DependencyStrategy::System {
                    let mut packages = self
                        .config
                        .get_known_package(&package)
                        .map(|info| info.pkg_config_packages.clone())
                        .filter(|packages| !packages.is_empty())
                        .unwrap_or_else(|| vec![dependency.clone()]);
                    let predicate = dependency.strip_prefix(&package).unwrap_or_default().trim();
                    if !(predicate.is_empty() || packages.len() == 1 && packages[0] == dependency) {
                        if packages.len() != 1 {
                            return Err(PlanError(format!(
                                "cannot rebind version predicate {dependency:?} to multiple system packages"
                            )));
                        }
                        let actual = package_name(&packages[0])?;
                        if packages[0].trim() != actual {
                            return Err(PlanError(format!(
                                "conflicting version predicates for system transitive {dependency:?}"
                            )));
                        }
                        packages[0] = format!("{actual}{predicate}");
                    }
                    for expression in &packages {
                        package_name(expression)?;
                    }
                    self.system(PkgConfigModule {
                        prefix: target_name(&package),
                        packages,
                        required: true,
                    })?
                } else {
                    self.resolve(&package, &dependency, Some(&package), strategy)?
                };
                deps.insert(target);
            }
            self.visiting.remove(&name);
            self.plan.sources.insert(
                name.clone(),
                PlannedSource {
                    info,
                    options,
                    deps: deps.into_iter().collect(),
                },
            );
            self.module("rules_foreign_cc", &self.config.versions.rules_foreign_cc)?;
        }
        Ok(format!("//third_party:{name}_source"))
    }
}

fn validate_name(name: &str) -> Result<()> {
    if name.is_empty()
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_-.+".contains(&byte))
        || name.starts_with('.')
    {
        return Err(PlanError(format!(
            "unsafe generated dependency name {name:?}"
        )));
    }
    Ok(())
}

fn package_name(expression: &str) -> Result<String> {
    let name = expression
        .split(['<', '>', '='])
        .next()
        .unwrap_or_default()
        .trim();
    validate_name(name)?;
    Ok(name.into())
}

// Numeric dotted versions cover the supported pkg-config predicates; other syntax fails explicitly.
fn validate_version(expression: &str, package: &str, selected: &str) -> Result<()> {
    let expression = expression.trim();
    let predicate = expression.strip_prefix(package).unwrap_or_default().trim();
    if predicate.is_empty() {
        return Ok(());
    }
    let (operator, required) = [">=", "<=", "=", ">", "<"]
        .iter()
        .find_map(|operator| {
            predicate
                .strip_prefix(operator)
                .map(|version| (*operator, version.trim()))
        })
        .ok_or_else(|| PlanError(format!("unsupported version predicate {expression:?}")))?;
    let parse = |version: &str| {
        version
            .split('.')
            .map(|part| {
                part.parse::<u64>().map_err(|_| {
                    PlanError(format!(
                        "cannot compare non-numeric version {version:?} for {expression:?}"
                    ))
                })
            })
            .collect::<Result<Vec<_>>>()
    };
    // BCR's .bcr.N suffix is a packaging revision, not an upstream API version.
    let upstream = selected
        .split_once(".bcr.")
        .filter(|(_, revision)| {
            !revision.is_empty() && revision.bytes().all(|byte| byte.is_ascii_digit())
        })
        .map_or(selected, |(upstream, _)| upstream);
    let actual = parse(upstream)?;
    let wanted = parse(required)?;
    let valid = match operator {
        ">=" => actual >= wanted,
        "<=" => actual <= wanted,
        "=" => actual == wanted,
        ">" => actual > wanted,
        "<" => actual < wanted,
        _ => false,
    };
    if !valid {
        return Err(PlanError(format!(
            "selected version {selected:?} does not satisfy {expression:?}"
        )));
    }
    Ok(())
}

fn validate_source(name: &str, info: &KnownPackageInfo) -> Result<SourceOptions> {
    if !(info.url.starts_with("https://") || info.url.starts_with("file:///"))
        || info.url.chars().any(char::is_control)
        || info.sha256.len() != 64
        || !info.sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(PlanError(format!(
            "source {name:?} requires an https/file archive URL and a 64-digit SHA256"
        )));
    }
    let relative = |value: &str| {
        !value.starts_with('/')
            && !value.contains('\\')
            && !value.chars().any(char::is_control)
            && value
                .split('/')
                .all(|component| component != ".." && component != ".")
    };
    if !relative(&info.strip_prefix)
        || info.out_static_libs.is_empty() && info.out_shared_libs.is_empty()
        || info
            .out_static_libs
            .iter()
            .chain(&info.out_shared_libs)
            .any(|output| output.is_empty() || !relative(output))
    {
        return Err(PlanError(format!(
            "source {name:?} requires relative strip/output paths and at least one library output"
        )));
    }
    match info.build_system {
        SourceBuildSystem::Meson => {
            let mut options = BTreeMap::new();
            for option in &info.build_options {
                let value = option.strip_prefix("-D").unwrap_or(option);
                let (key, value) = value.split_once('=').unwrap_or((value, "true"));
                validate_name(key)?;
                if options.insert(key.into(), value.into()).is_some() {
                    return Err(PlanError(format!(
                        "duplicate Meson option {key:?} for {name:?}"
                    )));
                }
            }
            Ok(SourceOptions::Meson(options))
        }
        SourceBuildSystem::Cmake => Ok(SourceOptions::Cmake(info.build_options.clone())),
        SourceBuildSystem::Autotools => Ok(SourceOptions::Autotools(info.build_options.clone())),
    }
}
