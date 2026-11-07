//! Native link decisions shared by preparation and rendering; no host IO.

use std::collections::{BTreeSet, HashMap};

use rebaze_cmake::{CMakeProject, Library};

use crate::config::{DependencyMapping, MigrationConfig};
use crate::dependency_plan::DependencyPlan;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum NativeLinkError {
    #[error(
        "target {target:?} requires unresolved dependency {dependency:?}; configure its mapping in rebaze.toml or explicitly add it to mappings.ignored_libraries"
    )]
    Unresolved { target: String, dependency: String },
    #[error(
        "target {target:?} has a comment instead of a mapping for {dependency:?}; configure a Bazel label or explicitly omit the dependency"
    )]
    CommentMapping { target: String, dependency: String },
}

pub fn validate(
    project: &CMakeProject,
    config: &MigrationConfig,
    plan: &DependencyPlan,
) -> Result<(), NativeLinkError> {
    for (name, links) in project
        .libraries
        .iter()
        .map(|library| (&library.name, &library.link_libraries))
        .chain(
            project
                .executables
                .iter()
                .map(|exe| (&exe.name, &exe.link_libraries)),
        )
    {
        dependencies(
            name,
            links,
            &project.libraries,
            &project.aliases,
            config,
            plan,
        )?;
    }
    Ok(())
}

pub fn dependencies(
    target: &str,
    links: &[String],
    libraries: &[Library],
    aliases: &HashMap<String, String>,
    config: &MigrationConfig,
    plan: &DependencyPlan,
) -> Result<Vec<String>, NativeLinkError> {
    let internal_names: BTreeSet<_> = libraries
        .iter()
        .map(|library| library.name.as_str())
        .collect();
    let mut seen = BTreeSet::new();
    let mut deps = Vec::new();
    for link in links {
        let resolved = aliases.get(link).map_or(link.as_str(), String::as_str);
        // Preserve the existing one-hop/namespace behavior until target identity
        // and alias graph validation are handled by the following work package.
        let local_name = resolved.rsplit("::").next().unwrap_or(resolved);
        let mapping = if internal_names.contains(local_name) {
            DependencyMapping::Mapped(format!(":{}", local_name.replace('-', "_")))
        } else if let Some(label) = plan.managed_link(resolved) {
            DependencyMapping::Mapped(label)
        } else {
            config.map_dependency(resolved)
        };
        match mapping {
            DependencyMapping::Mapped(label) => {
                if label.trim_start().starts_with('#') {
                    return Err(NativeLinkError::CommentMapping {
                        target: target.into(),
                        dependency: link.clone(),
                    });
                }
                if seen.insert(label.clone()) {
                    deps.push(label);
                }
            }
            DependencyMapping::Omitted(_) => {}
            DependencyMapping::Unresolved => {
                return Err(NativeLinkError::Unresolved {
                    target: target.into(),
                    dependency: link.clone(),
                });
            }
        }
    }
    // Keep the existing managed strategy edges until the per-target ownership
    // replacement has its own compatibility coverage.
    for name in plan.aliases.keys() {
        let label = format!("//third_party:{name}");
        if seen.insert(label.clone()) {
            deps.push(label);
        }
    }
    Ok(deps)
}
