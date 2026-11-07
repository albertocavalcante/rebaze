//! Third-party dependency configuration and generation.
//!
//! This module generates extensible BUILD files for third-party dependencies,
//! supporting multiple resolution strategies:
//!
//! - `system`: Use system-installed libraries via new_local_repository (non-hermetic)
//! - `source`: Build from source using rules_foreign_cc (hermetic)
//! - `conan`: Use Conan package manager
//! - `vcpkg`: Use vcpkg package manager
//! - `prebuilt`: Use pre-built libraries from filesystem
//! - `bazelle`: Use bazelle/gazelle_cc to generate BUILD files
//! - `custom`: User-defined resolution

use crate::config::{KnownPackageInfo, MigrationConfig, SourceBuildSystem};
use crate::starlark::{
    Alias, CcLibrary, Cmake, ConfigureMake, File, FunctionCall, Identifier, Load, Meson,
    RenderResult,
};
use crate::{DiscoverySnapshot, GenerationResult};
use rebaze_cmake::PkgConfigModule;
use serde::ser::SerializeMap;
use serde::{Serialize, Serializer};
use std::collections::{BTreeMap, BTreeSet};

/// Deduplicate pkg-config modules by their normalized target name.
///
/// Returns a new `Vec` with duplicates removed, preserving the order of first occurrence.
/// Uses BTreeSet for deterministic deduplication behavior.
pub fn deduplicate_modules(modules: &[PkgConfigModule]) -> Vec<PkgConfigModule> {
    let mut seen = BTreeSet::new();
    modules
        .iter()
        .filter(|pkg| {
            let target_name = crate::discovery::target_name(&pkg.prefix);
            seen.insert(target_name)
        })
        .cloned()
        .collect()
}

/// Dependency resolution strategy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DepsStrategy {
    /// Use BCR modules via bazel_dep (hermetic, recommended)
    /// Dependencies are fetched from Bazel Central Registry.
    Bcr,
    /// Use system libraries via new_local_repository (non-hermetic, but works)
    /// This is the default since it works out of the box when deps are installed.
    #[default]
    System,
    /// Build from source using rules_foreign_cc (hermetic, recommended for production)
    Source,
    /// Use Conan package manager
    Conan,
    /// Use vcpkg package manager
    Vcpkg,
    /// Use pre-built libraries from filesystem
    Prebuilt,
    /// Use bazelle/gazelle_cc to generate BUILD files for deps
    Bazelle,
    /// User provides custom resolution
    Custom,
}

/// Configuration for third-party dependency generation.
#[derive(Debug, Clone, Default)]
pub struct ThirdPartyConfig {
    /// Default strategy for resolving dependencies
    pub default_strategy: DepsStrategy,
    /// Path to prebuilt libraries (for Prebuilt strategy)
    pub prebuilt_path: Option<String>,
    /// Conan profile to use
    pub conan_profile: Option<String>,
    /// vcpkg triplet to use
    pub vcpkg_triplet: Option<String>,
}

/// Generate aliases and system fallback targets.
pub fn generate_build_file(
    modules: &[PkgConfigModule],
    config: &ThirdPartyConfig,
    discovery: &DiscoverySnapshot,
) -> GenerationResult<String> {
    Ok(build_file(modules, config, discovery)?.render()?)
}

pub(crate) fn build_file(
    modules: &[PkgConfigModule],
    _config: &ThirdPartyConfig,
    discovery: &DiscoverySnapshot,
) -> GenerationResult<File> {
    let mut file = File::new();
    file.load(Load {
        bzl: "@rules_cc//cc:defs.bzl".into(),
        items: ["cc_library".into()].into(),
    })?;
    file.load(Load {
        bzl: "//third_party:config.bzl".into(),
        items: ["resolve_dep".into()].into(),
    })?;
    file.statement(&crate::starlark::Package {
        default_visibility: vec!["//visibility:public".into()],
    })?;
    file.comment("Third-party aliases and system fallbacks; system libraries are non-hermetic.");
    for package in modules {
        let name = crate::discovery::target_name(&package.prefix);
        file.comment(&format!(
            "{} -> {}",
            package.prefix,
            package.packages.join(", ")
        ));
        file.statement(&Alias {
            name: name.clone(),
            actual: FunctionCall {
                name: "resolve_dep",
                args: vec![name.clone()],
            },
        })?;
        let linkopts = if let Some(info) = discovery.package(package)? {
            // Absolute include paths belong in the system repository, not cc_library.copts.
            info.linkopts
                .iter()
                .filter(|flag| flag.starts_with("-l") || flag.starts_with("-L"))
                .cloned()
                .collect()
        } else {
            file.comment(&format!(
                "TODO: Review flags from pkg-config --cflags --libs {}",
                package.packages.join(" ")
            ));
            generate_system_linkopts(&package.packages)
        };
        file.statement(&CcLibrary {
            name: format!("{name}_system"),
            linkopts,
            ..CcLibrary::default()
        })?;
    }
    Ok(file)
}

struct DependencyConfiguration<'a> {
    packages: &'a [String],
}

impl Serialize for DependencyConfiguration<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(1))?;
        map.serialize_entry("pkg_config", self.packages)?;
        map.end()
    }
}

/// Serialize data separately from the fixed dependency resolver program.
pub fn generate_config_bzl(
    modules: &[PkgConfigModule],
    config: &ThirdPartyConfig,
) -> RenderResult<String> {
    let strategy = match config.default_strategy {
        DepsStrategy::Bcr => "bcr",
        DepsStrategy::System => "system",
        DepsStrategy::Source => "source",
        DepsStrategy::Conan => "conan",
        DepsStrategy::Vcpkg => "vcpkg",
        DepsStrategy::Prebuilt => "prebuilt",
        DepsStrategy::Bazelle => "bazelle",
        DepsStrategy::Custom => "custom",
    };
    let mut file = File::new();
    file.docstring(
        "Third-party dependency configuration. Edit the tables to select resolution strategies.",
    )?;
    file.assign(&Identifier::new("DEFAULT_STRATEGY")?, &strategy)?;
    let dependencies: BTreeMap<_, _> = modules
        .iter()
        .map(|package| {
            (
                crate::discovery::target_name(&package.prefix),
                DependencyConfiguration {
                    packages: &package.packages,
                },
            )
        })
        .collect();
    file.assign(&Identifier::new("DEPS_CONFIG")?, &dependencies)?;
    file.helper_source(include_str!("templates/config.bzl"));
    file.render()
}

/// Serialize the single package catalog as Starlark dictionaries.
struct SourceMetadata<'a>(&'a KnownPackageInfo);

impl Serialize for SourceMetadata<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let info = self.0;
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("url", &info.url)?;
        map.serialize_entry("sha256", &info.sha256)?;
        map.serialize_entry("strip_prefix", &info.strip_prefix)?;
        map.serialize_entry("build_system", &info.build_system)?;
        map.serialize_entry("out_static_libs", &info.out_static_libs)?;
        if !info.deps.is_empty() {
            map.serialize_entry("deps", &info.deps)?;
        }
        if !info.build_options.is_empty() {
            map.serialize_entry("meson_options", &info.build_options)?;
        }
        map.end()
    }
}

enum SourceConfiguration<'a> {
    Known(SourceMetadata<'a>),
    Unconfigured,
}

impl Serialize for SourceConfiguration<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Known(info) => info.serialize(serializer),
            Self::Unconfigured => serializer.serialize_map(Some(0))?.end(),
        }
    }
}

pub(crate) fn source_packages(
    modules: &[PkgConfigModule],
    config: &MigrationConfig,
) -> BTreeSet<String> {
    let mut packages = BTreeSet::new();
    let mut pending: Vec<_> = modules
        .iter()
        .map(|package| crate::discovery::target_name(&package.prefix))
        .collect();
    while let Some(name) = pending.pop() {
        if !packages.insert(name.clone()) {
            continue;
        }
        pending.extend(config.get_transitive_deps(&name));
        if let Some(info) = config.get_known_package(&name) {
            pending.extend(info.deps.iter().cloned());
        }
    }
    packages
}

/// Generate source metadata and its fixed module-extension implementation.
pub fn generate_source_bzl(modules: &[PkgConfigModule]) -> RenderResult<String> {
    source_bzl(modules, &MigrationConfig::default())
}

pub(crate) fn source_bzl(
    modules: &[PkgConfigModule],
    config: &MigrationConfig,
) -> RenderResult<String> {
    let mut file = File::new();
    file.docstring("Source dependency definitions. Configure unknown packages before enabling the source strategy.")?;
    let sources: BTreeMap<_, _> = source_packages(modules, config)
        .into_iter()
        .map(|name| {
            if let Some(info) = config.get_known_package(&name) {
                (name, SourceConfiguration::Known(SourceMetadata(info)))
            } else {
                (name, SourceConfiguration::Unconfigured)
            }
        })
        .collect();
    file.assign(&Identifier::new("SOURCES")?, &sources)?;
    file.helper_source(include_str!("templates/source.bzl"));
    file.render()
}

pub fn generate_source_build_targets(modules: &[PkgConfigModule]) -> RenderResult<String> {
    source_build_targets(modules, &MigrationConfig::default())?.render()
}

pub(crate) fn source_build_targets(
    modules: &[PkgConfigModule],
    config: &MigrationConfig,
) -> RenderResult<File> {
    let mut file = File::new();
    file.load(Load {
        bzl: "@rules_foreign_cc//foreign_cc:defs.bzl".into(),
        items: ["cmake".into(), "configure_make".into(), "meson".into()].into(),
    })?;
    for package in source_packages(modules, config) {
        let Some(info) = config.get_known_package(&package) else {
            file.comment(&format!(
                "TODO: Configure source build for {package} in SOURCES and add a target here."
            ));
            continue;
        };
        let name = format!("{package}_source");
        let source = format!("@{package}_src//:all");
        let dependencies = info
            .deps
            .iter()
            .cloned()
            .chain(config.get_transitive_deps(&package))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .map(|name| format!(":{name}_source"))
            .collect::<Vec<_>>();
        match info.build_system {
            SourceBuildSystem::Meson => {
                let options = info
                    .build_options
                    .iter()
                    .map(|option| {
                        let option = option.strip_prefix("-D").unwrap_or(option);
                        let (name, value) = option.split_once('=').unwrap_or((option, "true"));
                        (name.to_owned(), value.to_owned())
                    })
                    .collect();
                file.statement(
                    &Meson::new(name, source)
                        .with_static_libs(info.out_static_libs.clone())
                        .with_options(options)
                        .with_deps(dependencies)
                        .with_visibility_public(),
                )?;
            }
            SourceBuildSystem::Cmake => file.statement(
                &Cmake::new(name, source)
                    .with_static_libs(info.out_static_libs.clone())
                    .with_deps(dependencies)
                    .with_visibility_public(),
            )?,
            SourceBuildSystem::Autotools => file.statement(
                &ConfigureMake::new(name, source)
                    .with_static_libs(info.out_static_libs.clone())
                    .with_deps(dependencies)
                    .with_visibility_public(),
            )?,
        }
    }
    Ok(file)
}

#[derive(Default)]
struct SystemConfiguration {
    path: Option<String>,
    include_dirs: Vec<String>,
    lib_dirs: Vec<String>,
    libs: Vec<String>,
}

impl Serialize for SystemConfiguration {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(None)?;
        if let Some(path) = &self.path {
            map.serialize_entry("path", path)?;
            map.serialize_entry("include_dirs", &self.include_dirs)?;
            map.serialize_entry("lib_dirs", &self.lib_dirs)?;
            map.serialize_entry("libs", &self.libs)?;
        }
        map.end()
    }
}

/// Snapshot observed pkg-config state into a table; keep the extension program fixed.
pub fn generate_system_deps_bzl(
    modules: &[PkgConfigModule],
    discovery: &DiscoverySnapshot,
) -> GenerationResult<String> {
    let mut libraries = BTreeMap::new();
    for package in modules {
        let configuration = discovery
            .package(package)?
            .and_then(|info| info.system.as_ref())
            .map_or_else(SystemConfiguration::default, |info| SystemConfiguration {
                path: Some(info.prefix.clone()),
                include_dirs: info.include_dirs.clone(),
                lib_dirs: info.lib_dirs.clone(),
                libs: info.libs.clone(),
            });
        libraries.insert(
            crate::discovery::target_name(&package.prefix),
            configuration,
        );
    }
    let mut file = File::new();
    file.docstring("System library snapshot from pkg-config (non-hermetic). Review paths for the target system.")?;
    file.assign(&Identifier::new("SYSTEM_LIBS")?, &libraries)?;
    file.helper_source(include_str!("templates/system_deps.bzl"));
    Ok(file.render()?)
}

/// Generate linkopts for system libraries.
fn generate_system_linkopts(packages: &[String]) -> Vec<String> {
    let mut linkopts = Vec::new();

    for pkg in packages {
        let base = pkg.split(">=").next().unwrap_or(pkg).trim();

        match base {
            "glib-2.0" => linkopts.push("-lglib-2.0".to_string()),
            "gobject-2.0" => linkopts.push("-lgobject-2.0".to_string()),
            "gio-2.0" => linkopts.push("-lgio-2.0".to_string()),
            "gthread-2.0" => linkopts.push("-lgthread-2.0".to_string()),
            "gmodule-2.0" => linkopts.push("-lgmodule-2.0".to_string()),
            "libpeas-1.0" => linkopts.push("-lpeas-1.0".to_string()),
            "libdnf" => linkopts.push("-ldnf".to_string()),
            "smartcols" => linkopts.push("-lsmartcols".to_string()),
            "libcurl" | "curl" => linkopts.push("-lcurl".to_string()),
            "openssl" => {
                linkopts.push("-lssl".to_string());
                linkopts.push("-lcrypto".to_string());
            }
            "zlib" => linkopts.push("-lz".to_string()),
            "libxml-2.0" => linkopts.push("-lxml2".to_string()),
            _ => {
                let lib_name = base
                    .strip_prefix("lib")
                    .unwrap_or(base)
                    .split('-')
                    .next()
                    .unwrap_or(base);
                linkopts.push(format!("-l{lib_name}"));
            }
        }
    }

    linkopts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_system_linkopts() {
        let packages = vec!["glib-2.0>=2.44.0".to_string()];
        let linkopts = generate_system_linkopts(&packages);
        assert_eq!(linkopts, vec!["-lglib-2.0"]);
    }

    #[test]
    fn test_deps_strategy_default() {
        let config = ThirdPartyConfig::default();
        assert_eq!(config.default_strategy, DepsStrategy::System);
    }
}
