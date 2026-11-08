//! Explicit project IO. Pure Gradle/Java parsing stays in the syntax adapters.
use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail};
use rebaze_input::{EntryKind, ReadError, ReadLimits, RootReader, SymlinkPolicy};

use crate::{GradleModule, GradleProject, java, parse_build_source, parser, project_path};

const MAX_MODULES: usize = 1024;

pub fn read_project(path: &Path, limits: ReadLimits) -> Result<GradleProject> {
    // Ambient canonicalization is part of selecting the root. Once opened, all
    // capture IO uses the capability, never this display pathname again.
    let root = path.canonicalize().context("resolve Gradle project root")?;
    let mut reader = RootReader::open(&root, limits, SymlinkPolicy::Reject)?;
    let settings = select_file(
        &mut reader,
        Path::new(""),
        "settings.gradle",
        "settings.gradle.kts",
    )?
    .context("No settings.gradle or settings.gradle.kts found")?;
    let (name, includes) = parser::settings(&reader.read_text(&settings)?)
        .with_context(|| format!("unsupported settings in {}", settings.display()))?;
    let name = name.unwrap_or_else(|| {
        root.file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned()
    });
    let paths = module_paths(includes)?;
    let mut modules = Vec::new();
    for project in paths {
        let directory = PathBuf::from(project.trim_start_matches(':').replace(':', "/"));
        reader.canonical_dir(&directory)?;
        let mut module = if let Some(build) =
            select_file(&mut reader, &directory, "build.gradle", "build.gradle.kts")?
        {
            parse_build_source(&reader.read_text(&build)?)
                .with_context(|| format!("unsupported build metadata in {}", build.display()))?
        } else {
            GradleModule::default()
        };
        module.path = project;
        module.sources = files(&mut reader, &directory, "src/main/java", true)?;
        module.resources = files(&mut reader, &directory, "src/main/resources", false)?;
        module.test_sources = files(&mut reader, &directory, "src/test/java", true)?;
        module.test_resources = files(&mut reader, &directory, "src/test/resources", false)?;
        for source in &module.test_sources {
            let text = reader.read_text(&directory.join(source))?;
            if let Some(class) = java::test_class(source, &text)? {
                module.test_classes.push(class);
            }
        }
        modules.push(module);
    }
    let maven_lock = reader
        .read_optional_text(Path::new("maven_install.json"))
        .map_err(|error| {
            let context = if matches!(&error, ReadError::NotRegular(_)) {
                "Maven lock must be a regular project file"
            } else {
                "read Maven lock as a bounded regular project file"
            };
            anyhow::Error::new(error).context(context)
        })?;
    Ok(GradleProject {
        name,
        path: root,
        modules,
        maven_lock,
    })
}

fn module_paths(includes: Vec<String>) -> Result<BTreeSet<String>> {
    let mut paths = BTreeSet::from([String::new()]);
    let mut explicit = BTreeSet::new();
    for include in includes {
        let include = project_path(&include)?;
        if include.is_empty() || !explicit.insert(include.clone()) {
            bail!("duplicate/empty included project {include:?}");
        }
        let mut ancestor = String::new();
        for part in include.trim_start_matches(':').split(':') {
            ancestor.push(':');
            ancestor.push_str(part);
            if !paths.contains(&ancestor) {
                if paths.len() >= MAX_MODULES {
                    bail!("Gradle module count budget exceeded: limit {MAX_MODULES}");
                }
                paths.insert(ancestor.clone());
            }
        }
    }
    Ok(paths)
}

fn select_file(
    reader: &mut RootReader,
    directory: &Path,
    groovy: &str,
    kotlin: &str,
) -> Result<Option<PathBuf>> {
    let groovy = directory.join(groovy);
    let kotlin = directory.join(kotlin);
    let mut present = Vec::new();
    for path in [&groovy, &kotlin] {
        match reader.kind(path)? {
            None => {}
            Some(EntryKind::File) => present.push(path.clone()),
            Some(_) => bail!(
                "Gradle build/settings must be regular files, without symlinks: {}",
                path.display()
            ),
        }
    }
    match present.len() {
        0 => Ok(None),
        1 => Ok(present.pop()),
        _ => bail!(
            "ambiguous Gradle files: {} and {}",
            groovy.display(),
            kotlin.display()
        ),
    }
}

fn optional_directory(reader: &mut RootReader, module: &Path, path: &Path) -> Result<bool> {
    let mut prefix = module.to_path_buf();
    for component in path.components() {
        let Component::Normal(name) = component else {
            bail!("invalid source-root component");
        };
        prefix.push(name);
        match reader.kind(&prefix)? {
            None => return Ok(false),
            Some(EntryKind::Directory) => {}
            Some(EntryKind::Symlink) => bail!(
                "source symlinks need explicit mapping: {}",
                prefix.display()
            ),
            Some(_) => bail!(
                "source directory components must be directories without symlinks: {}",
                prefix.display()
            ),
        }
    }
    Ok(true)
}

fn files(
    reader: &mut RootReader,
    module: &Path,
    directory: &str,
    java_only: bool,
) -> Result<Vec<String>> {
    if !optional_directory(reader, module, Path::new(directory))? {
        return Ok(Vec::new());
    }
    let mut pending = vec![module.join(directory)];
    let mut files = Vec::new();
    while let Some(directory) = pending.pop() {
        for entry in reader.directory_entries(&directory)? {
            let path = directory.join(entry.name());
            match entry.kind() {
                EntryKind::Directory => pending.push(path),
                EntryKind::File => {
                    if let Some(relative) = source_path(module, &path, java_only)? {
                        files.push(relative);
                    }
                }
                EntryKind::Symlink => {
                    bail!("source symlinks need explicit mapping: {}", path.display())
                }
                EntryKind::Other => bail!(
                    "special source entries need explicit mapping: {}",
                    path.display()
                ),
            }
        }
    }
    files.sort();
    Ok(files)
}

fn source_path(module: &Path, path: &Path, java_only: bool) -> Result<Option<String>> {
    if java_only && path.extension().is_none_or(|extension| extension != "java") {
        return Ok(None);
    }
    let relative = path
        .strip_prefix(module)?
        .to_str()
        .context("source paths must be UTF-8")?;
    if cfg!(unix) && relative.contains('\\') {
        bail!("literal backslashes in source filenames require explicit mapping");
    }
    Ok(Some(relative.replace('\\', "/")))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests;
