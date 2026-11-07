//! Validate native target identity and link intent without host IO.

use std::collections::{BTreeMap, BTreeSet};

use rebaze_cmake::CMakeProject;

use crate::config::{DependencyMapping, MigrationConfig};
use crate::dependency_plan::DependencyPlan;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum NativeLinkError {
    #[error(
        "target {target:?} requires unresolved dependency {dependency:?}; configure its mapping in rebaze.toml or explicitly add it to mappings.ignored_libraries"
    )]
    Unresolved { target: String, dependency: String },
    #[error(
        "target {target:?} uses alias {alias:?} with unresolved dependency {dependency:?}; configure the terminal dependency mapping in rebaze.toml or explicitly omit that dependency"
    )]
    AliasUnresolved {
        target: String,
        alias: String,
        dependency: String,
    },
    #[error(
        "target {target:?} has a comment instead of a mapping for {dependency:?}; configure a Bazel label or explicitly omit the dependency"
    )]
    CommentMapping { target: String, dependency: String },
    #[error(
        "native target {0:?} must use ASCII letters, digits, '_', '-', '.', or '+' and cannot be '.' or '..'"
    )]
    InvalidTarget(String),
    #[error("{current} collides with {previous} at generated target {name:?}")]
    Collision {
        name: String,
        previous: String,
        current: String,
    },
    #[error("alias {0:?} conflicts with a production target or has an invalid name")]
    InvalidAlias(String),
    #[error("native identity {0:?} collides with a prepared pkg-config link")]
    ManagedIdentityCollision(String),
    #[error("alias {name:?} is declared more than once ({first:?} and {second:?})")]
    DuplicateAlias {
        name: String,
        first: String,
        second: String,
    },
    #[error("native alias cycle includes {0:?}")]
    AliasCycle(String),
    #[error("target {target:?} cannot link executable dependency {dependency:?}")]
    ExecutableLink { target: String, dependency: String },
    #[error("no prepared native dependencies for target {0:?}")]
    MissingTarget(String),
}

/// One output namespace shared by production and generated test targets.
#[derive(Debug, Default)]
pub struct TargetNames(BTreeMap<String, String>);

impl TargetNames {
    pub fn register(&mut self, name: &str, origin: String) -> Result<String, NativeLinkError> {
        if !valid_name(name) {
            return Err(NativeLinkError::InvalidTarget(name.into()));
        }
        let normalized = name.replace('-', "_");
        if let Some(previous) = self.0.get(&normalized) {
            return Err(NativeLinkError::Collision {
                name: normalized,
                previous: previous.clone(),
                current: origin,
            });
        }
        self.0.insert(normalized.clone(), origin);
        Ok(normalized)
    }
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && !matches!(name, "." | "..")
        && name
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-' | b'.' | b'+'))
}

/// Dependencies are resolved once against validated target and alias identities.
#[derive(Debug)]
pub struct NativeLinkPlan {
    targets: BTreeMap<String, PlannedTarget>,
}

#[derive(Debug)]
pub struct PlannedTarget {
    pub name: String,
    pub dependencies: Vec<String>,
}

impl NativeLinkPlan {
    pub fn new(
        project: &CMakeProject,
        config: &MigrationConfig,
        plan: &DependencyPlan,
        names: &mut TargetNames,
    ) -> Result<Self, NativeLinkError> {
        let mut libraries = BTreeMap::new();
        let mut executables = BTreeSet::new();
        let mut target_names = BTreeMap::new();
        for library in &project.libraries {
            if plan.managed_link(&library.name).is_some() {
                return Err(NativeLinkError::ManagedIdentityCollision(
                    library.name.clone(),
                ));
            }
            let normalized =
                names.register(&library.name, format!("library {:?}", library.name))?;
            libraries.insert(library.name.clone(), format!(":{normalized}"));
            target_names.insert(library.name.clone(), normalized);
        }
        for executable in &project.executables {
            if plan.managed_link(&executable.name).is_some() {
                return Err(NativeLinkError::ManagedIdentityCollision(
                    executable.name.clone(),
                ));
            }
            let normalized = names.register(
                &executable.name,
                format!("executable {:?}", executable.name),
            )?;
            executables.insert(executable.name.clone());
            target_names.insert(executable.name.clone(), normalized);
        }
        let mut declarations = BTreeMap::new();
        for alias in &project.aliases {
            if !alias.name.split("::").all(valid_name)
                || libraries.contains_key(&alias.name)
                || executables.contains(&alias.name)
            {
                return Err(NativeLinkError::InvalidAlias(alias.name.clone()));
            }
            if plan.managed_link(&alias.name).is_some() {
                return Err(NativeLinkError::ManagedIdentityCollision(
                    alias.name.clone(),
                ));
            }
            if let Some(previous) = declarations.insert(alias.name.as_str(), alias.target.as_str())
            {
                return Err(NativeLinkError::DuplicateAlias {
                    name: alias.name.clone(),
                    first: previous.into(),
                    second: alias.target.clone(),
                });
            }
        }
        let aliases = resolve_aliases(&declarations)?;
        let resolver = Resolver {
            libraries: &libraries,
            executables: &executables,
            aliases: &aliases,
            config,
            plan,
        };
        // Invalid unused aliases are still invalid project intent. Their terminal
        // may be a configured external imported target omitted by extraction.
        for alias in aliases.keys() {
            resolver.link(alias, alias)?;
        }
        let targets = project
            .libraries
            .iter()
            .map(|library| (&library.name, &library.link_libraries))
            .chain(
                project
                    .executables
                    .iter()
                    .map(|exe| (&exe.name, &exe.link_libraries)),
            )
            .map(|(name, links)| {
                let mut seen = BTreeSet::new();
                let mut deps = Vec::new();
                for link in links {
                    if let DependencyMapping::Mapped(label) = resolver.link(name, link)?
                        && seen.insert(label.clone())
                    {
                        deps.push(label);
                    }
                }
                let output_name = target_names
                    .remove(name)
                    .ok_or_else(|| NativeLinkError::MissingTarget(name.clone()))?;
                Ok((
                    name.clone(),
                    PlannedTarget {
                        name: output_name,
                        dependencies: deps,
                    },
                ))
            })
            .collect::<Result<_, NativeLinkError>>()?;
        Ok(Self { targets })
    }

    pub fn target(&self, target: &str) -> Result<&PlannedTarget, NativeLinkError> {
        self.targets
            .get(target)
            .ok_or_else(|| NativeLinkError::MissingTarget(target.into()))
    }
}

/// Iterative, memoized traversal avoids call-stack dependence on alias depth.
fn resolve_aliases(
    aliases: &BTreeMap<&str, &str>,
) -> Result<BTreeMap<String, String>, NativeLinkError> {
    let mut resolved = BTreeMap::<String, String>::new();
    for alias in aliases.keys() {
        if resolved.contains_key(*alias) {
            continue;
        }
        let mut path = Vec::new();
        let mut seen = BTreeSet::new();
        let mut current = *alias;
        let terminal = loop {
            if let Some(terminal) = resolved.get(current) {
                break terminal.clone();
            }
            let Some(next) = aliases.get(current) else {
                break current.to_string();
            };
            if !seen.insert(current) {
                return Err(NativeLinkError::AliasCycle(current.into()));
            }
            path.push(current);
            current = next;
        };
        for name in path {
            resolved.insert(name.into(), terminal.clone());
        }
    }
    Ok(resolved)
}

struct Resolver<'a> {
    libraries: &'a BTreeMap<String, String>,
    executables: &'a BTreeSet<String>,
    aliases: &'a BTreeMap<String, String>,
    config: &'a MigrationConfig,
    plan: &'a DependencyPlan,
}

impl Resolver<'_> {
    fn link(&self, target: &str, link: &str) -> Result<DependencyMapping, NativeLinkError> {
        let resolved = self.aliases.get(link).map_or(link, String::as_str);
        let mapping = if let Some(label) = self.libraries.get(resolved) {
            DependencyMapping::Mapped(label.clone())
        } else if self.executables.contains(resolved) {
            return Err(NativeLinkError::ExecutableLink {
                target: target.into(),
                dependency: link.into(),
            });
        } else if let Some(label) = self.plan.managed_link(resolved) {
            DependencyMapping::Mapped(label)
        } else if resolved
            .split_once("::")
            .is_some_and(|(namespace, _)| namespace.eq_ignore_ascii_case("PkgConfig"))
        {
            // This namespace names prepared pkg-config requests, not arbitrary
            // aliases into a third_party package that might never be generated.
            DependencyMapping::Unresolved
        } else {
            self.config.map_dependency(resolved)
        };
        match &mapping {
            DependencyMapping::Mapped(label) if label.trim_start().starts_with('#') => {
                return Err(NativeLinkError::CommentMapping {
                    target: target.into(),
                    dependency: link.into(),
                });
            }
            DependencyMapping::Unresolved => {
                if resolved != link {
                    return Err(NativeLinkError::AliasUnresolved {
                        target: target.into(),
                        alias: link.into(),
                        dependency: resolved.into(),
                    });
                }
                return Err(NativeLinkError::Unresolved {
                    target: target.into(),
                    dependency: link.into(),
                });
            }
            _ => {}
        }
        Ok(mapping)
    }
}
