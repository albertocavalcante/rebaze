//! Bazel file generator for rebaze.

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::io::Write;
use std::path::Path;

// Re-export for use by other crates
pub use crate::config::MigrationConfig;
pub use crate::generator::BazelFile;
pub use crate::third_party::deduplicate_modules;
pub use bundle::{BundleError, OutputBundle, OutputPath};
pub use cmake_tests::TestError;
pub use dependency_plan::{DependencyStrategy, PlanError};
pub use discovery::DiscoverySnapshot;
pub use native_links::NativeLinkError;
pub use report::{
    DependencyDecision, DependencyResolution, Diagnostic, DiagnosticCode, DiagnosticOrigin,
    GenerationOutput, LoweringReport,
};

/// Failure to render a complete native output bundle.
#[derive(Debug, thiserror::Error)]
pub enum GenerationError {
    #[error(transparent)]
    Bundle(#[from] BundleError),
    #[error(transparent)]
    Source(#[from] rebaze_cmake::SourceIssue),
    #[error("cannot migrate Gradle Java project: {0}")]
    Gradle(String),
    #[error(transparent)]
    Config(#[from] config::ConfigError),
    #[error(transparent)]
    Render(#[from] rebaze_starlark::Error),
    #[error(transparent)]
    Plan(#[from] PlanError),
    #[error(transparent)]
    Discovery(#[from] discovery::SnapshotError),
    #[error(transparent)]
    Test(#[from] TestError),
    #[error(transparent)]
    NativeLink(#[from] NativeLinkError),
}

pub type GenerationResult<T> = std::result::Result<T, GenerationError>;

#[cfg(any(target_os = "linux", target_os = "macos"))]
pub mod apply;
mod bundle;
mod cmake_generator;
mod cmake_tests;
pub mod config;
#[cfg(test)]
mod config_build_tests;
#[cfg(test)]
mod dependency_build_tests;
mod dependency_plan;
#[cfg(test)]
mod dependency_plan_tests;
pub mod discovery;
pub mod filters;
#[cfg(test)]
mod generation_tests;
mod generator;
mod native_links;
mod report;
pub use rebaze_starlark as starlark;
mod third_party;

/// Generate Bazel files from a parsed Gradle project.
pub fn generate(
    project: &rebaze_gradle::GradleProject,
) -> GenerationResult<HashMap<String, String>> {
    Ok(generate_bundle(project)?.bundle.into_files())
}

/// Generate validated ordered JVM output without applying or verifying it.
pub fn generate_bundle(
    project: &rebaze_gradle::GradleProject,
) -> GenerationResult<GenerationOutput> {
    Ok(GenerationOutput {
        bundle: OutputBundle::new(generator::generate(project)?)?,
        report: LoweringReport::default(),
    })
}

/// Generate Bazel files from a parsed CMake project.
pub fn generate_from_cmake(
    project: &rebaze_cmake::CMakeProject,
    discovery: &DiscoverySnapshot,
) -> GenerationResult<HashMap<String, String>> {
    generate_from_cmake_with_config(project, &MigrationConfig::default(), discovery)
}

/// Generate Bazel files from a parsed CMake project with custom configuration.
pub fn generate_from_cmake_with_config(
    project: &rebaze_cmake::CMakeProject,
    config: &MigrationConfig,
    discovery: &DiscoverySnapshot,
) -> GenerationResult<HashMap<String, String>> {
    plan_cmake(project, config)?.render(discovery)
}

/// Borrowed project and configuration paired with their validated output intent.
/// Plan once, discover only `system_modules()`, then render the same preparation.
#[derive(Debug)]
pub struct CMakeGeneration<'a> {
    project: &'a rebaze_cmake::CMakeProject,
    config: &'a MigrationConfig,
    plan: dependency_plan::DependencyPlan,
    tests: Vec<cmake_tests::PlannedTest<'a>>,
    links: native_links::NativeLinkPlan,
}

pub fn plan_cmake<'a>(
    project: &'a rebaze_cmake::CMakeProject,
    config: &'a MigrationConfig,
) -> GenerationResult<CMakeGeneration<'a>> {
    config.validate_for(config::ConfigProfile::NativeCmake)?;
    if let Some(issue) = project.requires_evaluation.first() {
        return Err(issue.clone().into());
    }
    let plan = dependency_plan::DependencyPlan::new(project, config)?;
    let mut names = native_links::TargetNames::default();
    let links = native_links::NativeLinkPlan::new(project, config, &plan, &mut names)?;
    let tests = cmake_tests::plan(project, &mut names)?;
    Ok(CMakeGeneration {
        project,
        config,
        plan,
        tests,
        links,
    })
}

impl CMakeGeneration<'_> {
    pub fn system_modules(&self) -> &[rebaze_cmake::PkgConfigModule] {
        &self.plan.systems
    }

    pub fn render(
        &self,
        discovery: &DiscoverySnapshot,
    ) -> GenerationResult<HashMap<String, String>> {
        Ok(self.render_bundle(discovery)?.bundle.into_files())
    }

    /// Render immutable ordered intent and lowering decisions, without IO.
    pub fn render_bundle(
        &self,
        discovery: &DiscoverySnapshot,
    ) -> GenerationResult<GenerationOutput> {
        for module in &self.plan.systems {
            discovery.package(module)?;
        }
        let mut files = HashMap::from([
            (
                "MODULE.bazel".into(),
                cmake_generator::generate_module_bazel(self.project, self.config, &self.plan)?,
            ),
            (
                "BUILD.bazel".into(),
                cmake_generator::generate_root_build(
                    self.project,
                    self.config,
                    &self.links,
                    &self.tests,
                )?,
            ),
            (".bazelversion".into(), "9.0.0\n".into()),
            (
                ".bazelrc".into(),
                generate_cpp_bazelrc(self.project, self.config, &[]),
            ),
        ]);
        if !self.plan.aliases.is_empty() {
            let mut build = third_party::build_file(&self.plan)?;
            build.append(third_party::source_build_targets(&self.plan)?);
            files.insert("third_party/BUILD.bazel".into(), build.render()?);
            files.insert(
                "third_party/config.bzl".into(),
                third_party::config_bzl(&self.plan)?,
            );
            files.insert(
                "third_party/source.bzl".into(),
                third_party::source_bzl(&self.plan)?,
            );
            files.insert(
                "third_party/system_deps.bzl".into(),
                third_party::system_deps_bzl(&self.plan, discovery)?,
            );
        }
        Ok(GenerationOutput {
            bundle: OutputBundle::new(files)?,
            report: LoweringReport {
                dependencies: self.links.decisions().to_vec(),
            },
        })
    }
}

/// Supported C++ standards
const SUPPORTED_CXX_STANDARDS: &[&str] = &["98", "03", "11", "14", "17"];
/// Experimental C++ standards (work but may have issues)
const EXPERIMENTAL_CXX_STANDARDS: &[&str] = &["20", "23", "26"];

/// Generate a .bazelrc suitable for C/C++ projects.
// Bazelrc generation requires sequential config sections that form a logical unit
#[allow(clippy::option_if_let_else, clippy::too_many_lines)]
fn generate_cpp_bazelrc(
    project: &rebaze_cmake::CMakeProject,
    config: &MigrationConfig,
    system_includes: &[String],
) -> String {
    let mut lines = vec![
        "# Generated by rebaze - C/C++ project settings".to_string(),
        "# NOTE: Review and adjust these settings for your project".to_string(),
        String::new(),
    ];

    // Add system include paths detected from pkg-config
    if !system_includes.is_empty() {
        lines
            .push("# System include paths (detected from pkg-config during migration)".to_string());
        lines.push("# NOTE: These paths are machine-specific - adjust for your system".to_string());
        lines.push("# WARNING: Using system libraries makes builds non-hermetic".to_string());
        for path in system_includes {
            lines.push(format!("common --copt=-isystem{path}"));
        }
        lines.push(String::new());

        // Collect unique root directories for sandbox access
        let mut sandbox_paths: Vec<String> = Vec::new();
        for path in system_includes {
            // Find the Homebrew/system root (e.g., /opt/homebrew, /usr/local, /usr)
            if let Some(root) = find_sandbox_root(path)
                && !sandbox_paths.contains(&root)
            {
                sandbox_paths.push(root);
            }
        }

        if !sandbox_paths.is_empty() {
            lines.push("# Sandbox configuration for system library access".to_string());
            lines.push("# This allows Bazel's sandbox to access system include paths".to_string());
            for path in &sandbox_paths {
                lines.push(format!("build --sandbox_add_mount_pair={path}"));
            }
            lines.push(String::new());
        }
    }

    // Detect C vs C++ from project languages
    let has_cxx = project.languages.iter().any(|l| l == "CXX" || l == "C++");
    let has_c = project.languages.iter().any(|l| l == "C");

    // C++ standard settings
    if has_cxx {
        let (cxx_std, note) = match &project.cxx_standard {
            Some(std) => {
                if SUPPORTED_CXX_STANDARDS.contains(&std.as_str()) {
                    (std.clone(), None)
                } else if EXPERIMENTAL_CXX_STANDARDS.contains(&std.as_str()) {
                    (
                        std.clone(),
                        Some(format!(
                            "# WARNING: C++{std} is experimental - verify toolchain support"
                        )),
                    )
                } else {
                    (
                        "17".to_string(),
                        Some(format!(
                            "# WARNING: Unknown standard '{}' was detected, using C++17",
                            std.escape_debug()
                        )),
                    )
                }
            }
            None => (config.build.default_cxx_standard.clone(), None),
        };

        if let Some(note) = note {
            lines.push(note);
        }
        lines.push(format!(
            "# C++ standard (detected: {})",
            project
                .cxx_standard
                .as_deref()
                .unwrap_or("not specified")
                .escape_debug()
        ));
        lines.push(format!("common --cxxopt=-std=c++{cxx_std}"));
        lines.push(format!("common --host_cxxopt=-std=c++{cxx_std}"));
        lines.push(String::new());
    }

    // C standard settings
    if has_c {
        let c_std = match &project.c_standard {
            Some(std) => {
                // Normalize common variants
                match std.as_str() {
                    "90" | "89" => "90",
                    "99" => "99",
                    "11" => "11",
                    "17" | "18" => "17",
                    "23" => "23",
                    _ => {
                        lines.push(format!(
                            "# WARNING: Unknown C standard '{}' detected, using C11",
                            std.escape_debug()
                        ));
                        "11"
                    }
                }
            }
            None => config.build.default_c_standard.as_str(),
        };

        lines.push(format!(
            "# C standard (detected: {})",
            project
                .c_standard
                .as_deref()
                .unwrap_or("not specified")
                .escape_debug()
        ));
        lines.push(format!("common --conlyopt=-std=c{c_std}"));
        lines.push(format!("common --host_conlyopt=-std=c{c_std}"));
        lines.push(String::new());
    }

    // Common settings (apply to all commands: build, test, run, query, etc.)
    lines.extend([
        "# Allow empty glob patterns - rebaze generates .h and .hpp patterns".to_string(),
        "# NOTE: Review glob patterns in BUILD.bazel and remove allow_empty if not needed"
            .to_string(),
        "common --incompatible_disallow_empty_glob=false".to_string(),
        String::new(),
        "# Enable position-independent code (required for shared libraries)".to_string(),
        "common --copt=-fPIC".to_string(),
        String::new(),
        "# Warning settings (adjust based on your project's warning policy)".to_string(),
        "common --copt=-Wall".to_string(),
        "common --copt=-Wextra".to_string(),
        String::new(),
        "# Test settings".to_string(),
        "test --test_output=errors".to_string(),
        String::new(),
        "# CI config".to_string(),
        "common:ci --color=yes".to_string(),
        "common:ci --curses=no".to_string(),
        String::new(),
    ]);

    lines.join("\n")
}

/// Find the root directory to mount in sandbox for a system path.
/// E.g., /opt/homebrew/Cellar/glib/2.86.3/include -> /opt/homebrew
fn find_sandbox_root(path: &str) -> Option<String> {
    let path = std::path::Path::new(path);

    // Common system roots to check
    let known_roots = [
        "/opt/homebrew",
        "/usr/local",
        "/Library/Developer/CommandLineTools",
        "/Applications/Xcode.app",
    ];

    for root in &known_roots {
        if path.starts_with(root) {
            return Some((*root).to_string());
        }
    }

    // For /usr paths, just use /usr
    if path.starts_with("/usr") {
        return Some("/usr".to_string());
    }

    // For other paths, use the first two components
    let mut components = path.components();
    if let (Some(c1), Some(c2)) = (components.next(), components.next()) {
        let root = std::path::PathBuf::from(c1.as_os_str()).join(c2.as_os_str());
        return Some(root.to_string_lossy().to_string());
    }

    None
}

/// Write generated Bazel files to disk after validating the whole path set.
///
/// Each file is staged and atomically replaced, preserving the original on write
/// failure. The batch is not a transaction: an IO error can leave earlier files
/// updated. Pending guarded recovery blocks writing. Callers must exclude
/// concurrent application and changes to destination directories.
pub fn write_files<S: std::hash::BuildHasher>(
    root: &Path,
    files: &HashMap<String, String, S>,
) -> Result<()> {
    let bundle = OutputBundle::new(
        files
            .iter()
            .map(|(path, content)| (path.clone(), content.clone())),
    )?;
    write_bundle(root, &bundle)
}

/// Apply validated intent with the existing per-file replacement contract.
/// Destination state is observed here, never during bundle construction.
///
/// Any pending guarded transaction blocks the complete write before mutation.
/// This preflight does not lock: callers must exclude concurrent application.
pub fn write_bundle(root: &Path, bundle: &OutputBundle) -> Result<()> {
    ensure_no_pending_application(root)?;
    // Inspect the complete destination set before changing any generated file.
    for (path, _) in bundle.iter() {
        let relative = path.as_path();
        let mut candidate = root.to_path_buf();
        for component in relative.components() {
            candidate.push(component);
            match std::fs::symlink_metadata(&candidate) {
                Ok(metadata) if metadata.file_type().is_symlink() => {
                    anyhow::bail!("Refusing to write through symlink: {}", candidate.display());
                }
                Ok(metadata) => {
                    if candidate == root.join(path.as_path()) && !metadata.is_file()
                        || candidate != root.join(path.as_path()) && !metadata.is_dir()
                    {
                        anyhow::bail!(
                            "Generated path conflicts with existing entry: {}",
                            candidate.display()
                        );
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(error)
                        .with_context(|| format!("Failed to inspect {}", candidate.display()));
                }
            }
        }
    }
    for (path, content) in bundle.iter() {
        let full_path = root.join(path.as_path());
        if let Some(parent) = full_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let parent = full_path.parent().context("Generated file has no parent")?;
        let builder = tempfile::Builder::new();
        #[cfg(unix)]
        let builder = {
            use std::os::unix::fs::PermissionsExt;
            let mut builder = builder;
            // Match ordinary file creation while respecting the process umask.
            builder.permissions(std::fs::Permissions::from_mode(0o666));
            builder
        };
        let mut staged = builder
            .tempfile_in(parent)
            .with_context(|| format!("Failed to stage {}", full_path.display()))?;
        staged
            .write_all(content.as_bytes())
            .with_context(|| format!("Failed to write {}", full_path.display()))?;
        if let Ok(metadata) = std::fs::metadata(&full_path) {
            staged.as_file().set_permissions(metadata.permissions())?;
        }
        staged
            .persist(&full_path)
            .with_context(|| format!("Failed to replace {}", full_path.display()))?;
        tracing::info!("Wrote {}", full_path.display());
    }
    Ok(())
}

fn ensure_no_pending_application(root: &Path) -> Result<()> {
    let directory = root.join(".rebaze-apply");
    match std::fs::symlink_metadata(&directory) {
        Ok(metadata) => anyhow::ensure!(
            metadata.is_dir(),
            "Recovery namespace must be a directory, not a symlink or other entry: {}",
            directory.display()
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(error).with_context(|| {
                format!(
                    "Failed to inspect recovery namespace {}",
                    directory.display()
                )
            });
        }
    }
    let active = directory.join("active");
    match std::fs::symlink_metadata(&active) {
        Ok(_) => anyhow::bail!(
            "Guarded application is pending at {}; explicitly recover or inspect the retained data before writing generated output",
            active.display()
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| {
            format!(
                "Failed to inspect guarded application state {}",
                active.display()
            )
        }),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn case_aliases_fail_before_any_output_creation() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("outputs");
        let files = HashMap::from([
            ("App/BUILD.bazel".into(), "first".into()),
            ("app/BUILD.bazel".into(), "second".into()),
        ]);
        assert!(write_files(&root, &files).is_err());
        assert!(!root.exists());
    }

    #[test]
    fn reject_invalid_batch_before_writing_valid_entries() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("project");
        std::fs::create_dir(&root).unwrap();
        for invalid in ["../escaped", "/absolute", "sub/../../escaped", ""] {
            let files = HashMap::from([
                ("BUILD.bazel".into(), "valid".into()),
                (invalid.into(), "invalid".into()),
            ]);
            assert!(write_files(&root, &files).is_err(), "{invalid}");
            assert_eq!(std::fs::read_dir(&root).unwrap().count(), 0);
        }
        assert!(!temp.path().join("escaped").exists());
    }

    #[test]
    fn reserved_namespaces_are_preserved_before_any_write() {
        for name in [".git", ".HG", ".SvN", ".ReBaZe-ApPlY"] {
            for prefix in ["", "nested/"] {
                let root = tempfile::tempdir().unwrap();
                let reserved = format!("{prefix}{name}/config");
                let metadata = root.path().join(&reserved);
                std::fs::create_dir_all(metadata.parent().unwrap()).unwrap();
                std::fs::write(&metadata, "private baseline").unwrap();
                let destination = root.path().join("BUILD.bazel");
                std::fs::write(&destination, "user baseline").unwrap();
                let result = write_files(
                    root.path(),
                    &HashMap::from([
                        ("BUILD.bazel".into(), "generated".into()),
                        ("new/BUILD.bazel".into(), "new file".into()),
                        (reserved.clone(), "replacement".into()),
                    ]),
                );
                assert!(result.is_err(), "accepted {reserved}");
                assert_eq!(std::fs::read(&destination).unwrap(), b"user baseline");
                assert_eq!(std::fs::read(&metadata).unwrap(), b"private baseline");
                assert!(!root.path().join("new").exists());
            }
        }
    }

    #[test]
    fn pending_application_entries_block_the_entire_bundle() {
        for kind in ["empty", "unknown", "file"] {
            let root = tempfile::tempdir().unwrap();
            let active = root.path().join(".rebaze-apply/active");
            std::fs::create_dir(active.parent().unwrap()).unwrap();
            if kind == "file" {
                std::fs::write(&active, "private record").unwrap();
            } else {
                std::fs::create_dir(&active).unwrap();
                if kind == "unknown" {
                    std::fs::write(active.join("manifest.json"), "{malformed").unwrap();
                }
            }
            let destination = root.path().join("BUILD.bazel");
            std::fs::write(&destination, "baseline").unwrap();
            let bundle = OutputBundle::new([
                ("BUILD.bazel".into(), "replacement".into()),
                ("new/BUILD.bazel".into(), "new file".into()),
            ])
            .unwrap();
            let error = write_bundle(root.path(), &bundle).unwrap_err();
            assert!(format!("{error:#}").contains("pending"), "{error:#}");
            assert!(format!("{error:#}").contains(&active.display().to_string()));
            assert_eq!(std::fs::read(&destination).unwrap(), b"baseline");
            assert!(!root.path().join("new").exists());
            if kind == "file" {
                assert_eq!(std::fs::read(&active).unwrap(), b"private record");
            } else if kind == "unknown" {
                assert_eq!(
                    std::fs::read(active.join("manifest.json")).unwrap(),
                    b"{malformed"
                );
            } else {
                assert_eq!(std::fs::read_dir(&active).unwrap().count(), 0);
            }
        }
    }

    #[test]
    fn invalid_recovery_directory_blocks_output() {
        let root = tempfile::tempdir().unwrap();
        let private = root.path().join(".rebaze-apply");
        std::fs::write(&private, "private baseline").unwrap();
        assert!(
            write_files(
                root.path(),
                &HashMap::from([("BUILD.bazel".into(), "generated".into())]),
            )
            .is_err()
        );
        assert_eq!(std::fs::read(&private).unwrap(), b"private baseline");
        assert!(!root.path().join("BUILD.bazel").exists());
    }

    #[cfg(unix)]
    #[test]
    fn recovery_symlinks_block_output_without_following_them() {
        use std::os::unix::fs::symlink;
        for private_symlink in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let outside = tempfile::tempdir().unwrap();
            let private = root.path().join(".rebaze-apply");
            if private_symlink {
                symlink(outside.path(), &private).unwrap();
            } else {
                std::fs::create_dir(&private).unwrap();
                symlink(outside.path().join("absent"), private.join("active")).unwrap();
            }
            assert!(
                write_files(
                    root.path(),
                    &HashMap::from([("new/BUILD.bazel".into(), "generated".into())]),
                )
                .is_err()
            );
            assert!(!root.path().join("new").exists());
            assert_eq!(std::fs::read_dir(outside.path()).unwrap().count(), 0);
            let link = if private_symlink {
                private
            } else {
                private.join("active")
            };
            assert_eq!(
                std::fs::read_link(link).unwrap(),
                if private_symlink {
                    outside.path().to_path_buf()
                } else {
                    outside.path().join("absent")
                }
            );
        }
    }

    #[test]
    fn saved_recovery_archives_and_ordinary_dotfiles_remain_usable() {
        let root = tempfile::tempdir().unwrap();
        let saved = root.path().join(".rebaze-apply/saved-old/before");
        std::fs::create_dir_all(saved.parent().unwrap()).unwrap();
        std::fs::write(&saved, "archived original").unwrap();
        let bundle = OutputBundle::new([
            (".bazelrc".into(), "build --jobs=2".into()),
            (".gitignore".into(), "bazel-*".into()),
            (".gitattributes".into(), "* text=auto".into()),
            (
                ".rebaze-apply-backup/BUILD.bazel".into(),
                "# valid sibling".into(),
            ),
        ])
        .unwrap();
        write_bundle(root.path(), &bundle).unwrap();
        for (path, content) in bundle.iter() {
            assert_eq!(
                std::fs::read_to_string(root.path().join(path.as_path())).unwrap(),
                content
            );
        }
        assert_eq!(std::fs::read(&saved).unwrap(), b"archived original");
    }

    #[cfg(unix)]
    #[test]
    fn reject_symlinked_output_directories_and_files() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("project");
        let outside = temp.path().join("outside");
        std::fs::create_dir(&root).unwrap();
        std::fs::create_dir(&outside).unwrap();
        std::fs::write(outside.join("BUILD.bazel"), "original").unwrap();
        std::os::unix::fs::symlink(&outside, root.join("sub")).unwrap();
        std::os::unix::fs::symlink(outside.join("BUILD.bazel"), root.join("BUILD.bazel")).unwrap();
        for path in ["sub/BUILD.bazel", "BUILD.bazel"] {
            assert!(write_files(&root, &HashMap::from([(path.into(), "changed".into())])).is_err());
        }
        assert_eq!(
            std::fs::read_to_string(outside.join("BUILD.bazel")).unwrap(),
            "original"
        );
    }

    #[cfg(unix)]
    #[test]
    fn new_output_permissions_match_normal_file_creation() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().unwrap();
        let reference = temp.path().join("reference");
        std::fs::write(&reference, "reference").unwrap();
        write_files(
            temp.path(),
            &HashMap::from([("BUILD.bazel".into(), "new".into())]),
        )
        .unwrap();
        let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&temp.path().join("BUILD.bazel")), mode(&reference));
    }

    #[test]
    fn hardlinked_output_is_replaced_without_changing_the_other_link() {
        let temp = tempfile::tempdir().unwrap();
        let original = temp.path().join("original");
        std::fs::write(&original, "original").unwrap();
        std::fs::hard_link(&original, temp.path().join("BUILD.bazel")).unwrap();
        write_files(
            temp.path(),
            &HashMap::from([("BUILD.bazel".into(), "new".into())]),
        )
        .unwrap();
        assert_eq!(std::fs::read_to_string(original).unwrap(), "original");
        assert_eq!(
            std::fs::read_to_string(temp.path().join("BUILD.bazel")).unwrap(),
            "new"
        );
    }

    #[test]
    fn conflicting_output_paths_fail_before_any_writes() {
        let temp = tempfile::tempdir().unwrap();
        let files = HashMap::from([
            ("sub".into(), "file".into()),
            ("sub/BUILD.bazel".into(), "build".into()),
        ]);
        assert!(write_files(temp.path(), &files).is_err());
        assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 0);
    }

    fn java_module(path: &str) -> rebaze_gradle::GradleModule {
        rebaze_gradle::GradleModule {
            path: path.into(),
            plugins: vec![rebaze_gradle::GradlePlugin {
                id: "java".into(),
                version: None,
            }],
            ..rebaze_gradle::GradleModule::default()
        }
    }

    #[test]
    fn leading_gradle_colon_generates_a_relative_subproject_path() {
        let project = rebaze_gradle::GradleProject {
            name: "example".into(),
            path: ".".into(),
            modules: vec![java_module(":app:lib")],
            maven_lock: None,
        };
        assert!(
            generate(&project)
                .unwrap()
                .contains_key("app/lib/BUILD.bazel")
        );
    }
}
