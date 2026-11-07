//! Render only the dependencies selected by the validated plan.

use std::collections::BTreeMap;

use rebaze_cmake::PkgConfigModule;
use serde::ser::SerializeMap;
use serde::{Serialize, Serializer};

use crate::dependency_plan::{DependencyPlan, SourceOptions};
use crate::starlark::{
    Alias, CcLibrary, Cmake, ConfigureMake, File, Identifier, Load, Meson, RenderResult,
};
use crate::{DiscoverySnapshot, GenerationResult};

/// Retained for discovery callers; generation validates conflicting requests first.
pub fn deduplicate_modules(
    modules: &[PkgConfigModule],
) -> Result<Vec<PkgConfigModule>, crate::discovery::SnapshotError> {
    let mut seen = BTreeMap::<String, Vec<String>>::new();
    let mut unique = Vec::new();
    for module in modules {
        let name = crate::discovery::target_name(&module.prefix);
        if let Some(packages) = seen.get(&name) {
            if packages != &module.packages {
                return Err(crate::discovery::SnapshotError::RequestMismatch { module: name });
            }
        } else {
            seen.insert(name, module.packages.clone());
            unique.push(module.clone());
        }
    }
    Ok(unique)
}

pub fn build_file(plan: &DependencyPlan) -> RenderResult<File> {
    let mut file = File::new();
    file.load(Load {
        bzl: "@rules_cc//cc:defs.bzl".into(),
        items: ["cc_library".into()].into(),
    })?;
    file.statement(&crate::starlark::Package {
        default_visibility: vec!["//visibility:public".into()],
    })?;
    for (name, alias) in &plan.aliases {
        if let [target] = alias.targets.as_slice() {
            file.statement(&Alias {
                name: name.clone(),
                actual: target.clone(),
            })?;
        } else {
            file.statement(&CcLibrary {
                name: name.clone(),
                deps: alias.targets.clone(),
                ..CcLibrary::default()
            })?;
        }
    }
    Ok(file)
}

pub fn config_bzl(plan: &DependencyPlan) -> RenderResult<String> {
    let mut file = File::new();
    file.docstring(
        "Resolved dependency plan. Change rebaze.toml and regenerate to change strategies.",
    )?;
    file.assign(&Identifier::new("DEPS_CONFIG")?, &plan.aliases)?;
    file.render()
}

struct SourceArchive<'a> {
    url: &'a str,
    sha256: &'a str,
    strip_prefix: &'a str,
}

impl Serialize for SourceArchive<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(3))?;
        map.serialize_entry("url", self.url)?;
        map.serialize_entry("sha256", self.sha256)?;
        map.serialize_entry("strip_prefix", self.strip_prefix)?;
        map.end()
    }
}

pub fn source_bzl(plan: &DependencyPlan) -> RenderResult<String> {
    let mut file = File::new();
    file.docstring("Active source archives, with checksums validated before generation.")?;
    let sources: BTreeMap<_, _> = plan
        .sources
        .iter()
        .map(|(name, source)| {
            (
                name,
                SourceArchive {
                    url: &source.info.url,
                    sha256: &source.info.sha256,
                    strip_prefix: &source.info.strip_prefix,
                },
            )
        })
        .collect();
    file.assign(&Identifier::new("SOURCES")?, &sources)?;
    file.helper_source(include_str!("templates/source.bzl"));
    file.render()
}

pub fn source_build_targets(plan: &DependencyPlan) -> RenderResult<File> {
    let mut file = File::new();
    if plan.sources.is_empty() {
        return Ok(file);
    }
    let rules = plan
        .sources
        .values()
        .map(|source| match source.options {
            SourceOptions::Meson(_) => "meson".into(),
            SourceOptions::Cmake(_) => "cmake".into(),
            SourceOptions::Autotools(_) => "configure_make".into(),
        })
        .collect();
    file.load(Load {
        bzl: "@rules_foreign_cc//foreign_cc:defs.bzl".into(),
        items: rules,
    })?;
    for (package, source) in &plan.sources {
        let info = &source.info;
        let name = format!("{package}_source");
        let archive = format!("@{package}_src//:all");
        // Static outputs take precedence. Shared-only packages specify the exact platform outputs.
        let shared = if info.out_static_libs.is_empty() {
            info.out_shared_libs.clone()
        } else {
            vec![]
        };
        match &source.options {
            SourceOptions::Meson(options) => {
                let mut rule = Meson::new(name, archive)
                    .with_static_libs(info.out_static_libs.clone())
                    .with_options(options.clone())
                    .with_deps(source.deps.clone())
                    .with_visibility_public();
                rule.out_shared_libs = shared;
                file.statement(&rule)?;
            }
            SourceOptions::Cmake(options) => {
                let mut rule = Cmake::new(name, archive)
                    .with_static_libs(info.out_static_libs.clone())
                    .with_deps(source.deps.clone())
                    .with_visibility_public();
                rule.generate_args.extend(options.clone());
                rule.out_shared_libs = shared;
                file.statement(&rule)?;
            }
            SourceOptions::Autotools(options) => {
                let mut rule = ConfigureMake::new(name, archive)
                    .with_static_libs(info.out_static_libs.clone())
                    .with_deps(source.deps.clone())
                    .with_visibility_public();
                rule.configure_options.clone_from(options);
                rule.out_shared_libs = shared;
                file.statement(&rule)?;
            }
        }
    }
    Ok(file)
}

struct SystemConfiguration<'a> {
    path: Option<&'a str>,
    include_dirs: &'a [String],
    cflags: &'a [String],
    linkopts: &'a [String],
}

impl Serialize for SystemConfiguration<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(None)?;
        if let Some(path) = self.path {
            map.serialize_entry("path", path)?;
        }
        map.serialize_entry("include_dirs", self.include_dirs)?;
        map.serialize_entry("cflags", self.cflags)?;
        map.serialize_entry("linkopts", self.linkopts)?;
        map.end()
    }
}

pub fn system_deps_bzl(
    plan: &DependencyPlan,
    discovery: &DiscoverySnapshot,
) -> GenerationResult<String> {
    let mut libraries = BTreeMap::new();
    for package in &plan.systems {
        let info = discovery.package(package)?;
        libraries.insert(
            crate::discovery::target_name(&package.prefix),
            SystemConfiguration {
                path: info
                    .and_then(|info| info.system.as_ref())
                    .map(|system| system.prefix.as_str()),
                include_dirs: info
                    .and_then(|info| info.system.as_ref())
                    .map_or(&[][..], |system| system.include_dirs.as_slice()),
                cflags: info.map_or(&[][..], |info| info.cflags.as_slice()),
                linkopts: info.map_or(&[][..], |info| info.linkopts.as_slice()),
            },
        );
    }
    let mut file = File::new();
    file.docstring(
        "Selected system observations (non-hermetic). Review paths and compile flag limitations.",
    )?;
    file.assign(&Identifier::new("SYSTEM_LIBS")?, &libraries)?;
    file.helper_source(include_str!("templates/system_deps.bzl"));
    Ok(file.render()?)
}
