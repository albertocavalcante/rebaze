//! Core migration logic for rebaze.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

pub mod config;
pub mod discovery;
mod migration;
pub mod model;
pub use migration::{migrate, migrate_with_report, validate, validate_with_report};
pub mod report;

// Re-export config types for convenience
pub use config::{BazelConfig, Config};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildGenerator {
    Auto,
    Native,
    Bazelle,
}

pub struct MigrateOptions<'a> {
    pub path: &'a str,
    pub from: Option<&'a str>,
    pub dry_run: bool,
    pub cmake_build_dir: Option<&'a str>,
    pub cmake_config: Option<&'a str>,
    pub cmake_file_api_only: bool,
    pub build_generator: BuildGenerator,
    pub bazelle_root: Option<&'a str>,
    pub bazelle_bin: Option<&'a str>,
    pub unsafe_mode: bool,
    /// Migration configuration (versions, mappings, filters, etc.)
    pub config: Option<Config>,
}

/// Analyze a project and detect its build system.
pub fn analyze(path: &str) -> Result<String> {
    let path = Path::new(path);

    if !path.exists() {
        anyhow::bail!("Path does not exist: {}", path.display());
    }

    let mut detected = Vec::new();

    // Check for CMake (priority for C/C++ projects)
    if path.join("CMakeLists.txt").exists() {
        detected.push("cmake");
    }

    // Check for Gradle
    if path.join("build.gradle").exists() || path.join("build.gradle.kts").exists() {
        detected.push("gradle");
    }

    // Check for Maven
    if path.join("pom.xml").exists() {
        detected.push("maven");
    }

    // Check for Makefile
    if path.join("Makefile").exists() || path.join("makefile").exists() {
        detected.push("make");
    }

    // Check for Cargo (Rust)
    if path.join("Cargo.toml").exists() {
        detected.push("cargo");
    }

    if detected.is_empty() {
        Ok("No supported build system detected".to_string())
    } else {
        Ok(format!("Detected build systems: {}", detected.join(", ")))
    }
}

/// Validate generated Bazel files.
pub struct ValidateOptions<'a> {
    pub path: &'a str,
    pub unsafe_mode: bool,
}

fn detect_build_system(path: &Path) -> Result<String> {
    // CMake first (C/C++ projects)
    if path.join("CMakeLists.txt").exists() {
        return Ok("cmake".to_string());
    }
    if path.join("build.gradle").exists() || path.join("build.gradle.kts").exists() {
        return Ok("gradle".to_string());
    }
    if path.join("pom.xml").exists() {
        return Ok("maven".to_string());
    }

    anyhow::bail!("Could not detect build system at {}", path.display())
}

fn parse_cmake_project(
    path: &Path,
    cmake_build_dir: Option<&str>,
    cmake_config: Option<&str>,
    cmake_file_api_only: bool,
) -> Result<(rebaze_cmake::CMakeProject, report::SourceEvidence)> {
    if let Some(build_dir) = cmake_build_dir {
        let options = rebaze_cmake_file_api::FileApiOptions {
            configuration: cmake_config.map(str::to_string),
        };
        match rebaze_cmake_file_api::capture_build_dir_with_options(Path::new(build_dir), &options)
        {
            Ok(capture) => {
                let project = capture.project;
                let observed = project
                    .path
                    .canonicalize()
                    .context("Failed to resolve File API source root")?;
                let expected = path.canonicalize()?;
                anyhow::ensure!(
                    observed == expected,
                    "configured File API source root {} does not match project root {}",
                    observed.display(),
                    expected.display()
                );
                Ok((
                    project,
                    report::SourceEvidence::CMakeFileApi {
                        build_dir: Path::new(build_dir).canonicalize()?,
                        requested_configuration: cmake_config.map(str::to_string),
                        selected_configuration: capture.selected_configuration,
                    },
                ))
            }
            Err(err) => {
                // A present reply is selected evidence: never hide invalid or
                // unsupported configured semantics behind a source-only model.
                if cmake_file_api_only
                    || !matches!(
                        err,
                        rebaze_cmake_file_api::FileApiError::NotGenerated { .. }
                    )
                {
                    return Err(err.into());
                }
                tracing::warn!(
                    "Failed to load cmake-file-api from {build_dir}: {err}. Falling back to parser."
                );
                Ok((
                    rebaze_cmake::parse(path).context("Failed to parse CMake project")?,
                    report::SourceEvidence::CMakeSource,
                ))
            }
        }
    } else {
        if cmake_file_api_only {
            anyhow::bail!("--cmake-file-api-only requires --cmake-build-dir");
        }
        Ok((
            rebaze_cmake::parse(path).context("Failed to parse CMake project")?,
            report::SourceEvidence::CMakeSource,
        ))
    }
}

fn resolve_build_generator(
    build_system: &str,
    requested: BuildGenerator,
    bazelle_available: bool,
) -> Result<BuildGenerator> {
    match requested {
        BuildGenerator::Auto => {
            if build_system == "cmake" && bazelle_available {
                Ok(BuildGenerator::Bazelle)
            } else {
                Ok(BuildGenerator::Native)
            }
        }
        BuildGenerator::Bazelle => {
            if bazelle_available {
                Ok(BuildGenerator::Bazelle)
            } else {
                anyhow::bail!("bazelle is not available; set --bazelle-root or --bazelle-bin")
            }
        }
        BuildGenerator::Native => Ok(BuildGenerator::Native),
    }
}

fn bazelle_available(bazelle_bin: Option<&str>, bazelle_root: Option<&str>) -> bool {
    if let Some(bin) = bazelle_bin {
        return Path::new(bin).is_file();
    }

    if let Some(root) = bazelle_root {
        let root = Path::new(root);
        return root.join("WORKSPACE").exists() || root.join("MODULE.bazel").exists();
    }

    // Presence is observed without executing a tool, including during preview.
    // Actual execution success belongs to the external-generation stage.
    let name = if cfg!(windows) {
        "bazelle.exe"
    } else {
        "bazelle"
    };
    std::env::var_os("PATH").is_some_and(|value| {
        std::env::split_paths(&value).any(|dir| executable_candidate(&dir.join(name)))
    })
}

fn executable_candidate(path: &Path) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn run_bazelle_generation(
    project_root: &Path,
    module_name: &str,
    languages: &[String],
    bazelle_root: Option<&str>,
    bazelle_bin: Option<&str>,
    dry_run: bool,
) -> Result<()> {
    let languages_arg = languages.join(",");
    if dry_run {
        return Ok(());
    }
    let bazelle = ensure_bazelle_binary(bazelle_root, bazelle_bin)?;

    let mut init_cmd = Command::new(&bazelle);
    init_cmd.current_dir(project_root);
    init_cmd.arg("init");
    init_cmd.arg("--languages").arg(&languages_arg);
    init_cmd.arg("--name").arg(module_name);
    run_command(init_cmd, "bazelle init")?;

    let mut update_cmd = Command::new(&bazelle);
    update_cmd.current_dir(project_root);
    update_cmd.arg("update");
    update_cmd.arg("--languages").arg(&languages_arg);
    run_command(update_cmd, "bazelle update")?;

    Ok(())
}

fn ensure_bazelle_binary(bazelle_root: Option<&str>, bazelle_bin: Option<&str>) -> Result<PathBuf> {
    if let Some(bin) = bazelle_bin {
        let path = PathBuf::from(bin);
        if path.is_file() {
            return path
                .canonicalize()
                .context("Failed to resolve bazelle binary");
        }
        anyhow::bail!("bazelle binary not found at {}", path.display());
    }

    if let Some(root) = bazelle_root {
        let root = Path::new(root)
            .canonicalize()
            .context("Failed to resolve bazelle root")?;

        let status = Command::new("bazel")
            .current_dir(&root)
            .arg("build")
            .arg("//cmd/bazelle")
            .status()
            .context("Failed to build bazelle")?;
        if !status.success() {
            anyhow::bail!("bazelle build failed");
        }

        let bin = root.join("bazel-bin/cmd/bazelle/bazelle_/bazelle");
        if bin.is_file() {
            return Ok(bin);
        }

        anyhow::bail!("bazelle binary not found after build at {}", bin.display());
    }

    Ok(PathBuf::from("bazelle"))
}

fn bazelle_languages_for_cmake(_project: &rebaze_cmake::CMakeProject) -> Vec<String> {
    vec!["cc".to_string()]
}

fn write_bazel_version(root: &Path) -> Result<()> {
    let path = root.join(".bazelversion");
    let desired = "9.0.0\n";

    if path.exists() {
        let content = std::fs::read_to_string(&path)?;
        let version = content.trim();
        if version.split('.').next() != Some("9") {
            anyhow::bail!(
                "Unsupported Bazel version in {}: {} (rebaze supports Bazel 9 only)",
                path.display(),
                version.trim()
            );
        }
        return Ok(());
    }

    std::fs::write(path, desired)?;
    Ok(())
}

fn write_cpp_bazelrc(root: &Path) -> Result<()> {
    let path = root.join(".bazelrc");
    if path.exists() {
        return Ok(());
    }

    let content = cpp_bazelrc_template();

    std::fs::write(path, content)?;
    Ok(())
}

const fn cpp_bazelrc_template() -> &'static str {
    r"# Generated by rebaze - C++ project settings

# Use C++17 by default
build --cxxopt=-std=c++17
build --host_cxxopt=-std=c++17

# Enable position-independent code
build --copt=-fPIC

# Warning settings
build --copt=-Wall
build --copt=-Wextra

# Test settings
test --test_output=errors

# CI config
build:ci --color=yes
build:ci --curses=no
"
}

fn bazelle_preview(
    root: &Path,
    name: &str,
    languages: &[String],
    bazelle_root: Option<&str>,
    bazelle_bin: Option<&str>,
) -> Result<report::MigrationPreview> {
    use report::{ExistingFilePolicy, MigrationPreview, ProcessPreview, ScaffoldingPreview};
    let mut commands = Vec::new();
    let program = if let Some(bin) = bazelle_bin {
        Path::new(bin)
            .canonicalize()
            .context("Failed to resolve bazelle binary")?
    } else if let Some(tool_root) = bazelle_root {
        let tool_root = Path::new(tool_root)
            .canonicalize()
            .context("Failed to resolve bazelle root")?;
        commands.push(ProcessPreview {
            program: "bazel".into(),
            args: vec!["build".into(), "//cmd/bazelle".into()],
            cwd: tool_root.clone(),
        });
        tool_root.join("bazel-bin/cmd/bazelle/bazelle_/bazelle")
    } else {
        PathBuf::from("bazelle")
    };
    commands.push(ProcessPreview {
        program: program.clone(),
        args: vec![
            "init".into(),
            "--languages".into(),
            languages.join(","),
            "--name".into(),
            name.into(),
        ],
        cwd: root.into(),
    });
    commands.push(ProcessPreview {
        program,
        args: vec!["update".into(), "--languages".into(), languages.join(",")],
        cwd: root.into(),
    });
    let scaffolding = vec![
        ScaffoldingPreview {
            path: rebaze_bazel::OutputPath::new(".bazelversion".into())?,
            content: "9.0.0\n".into(),
            existing: ExistingFilePolicy::RequireBazel9,
        },
        ScaffoldingPreview {
            path: rebaze_bazel::OutputPath::new(".bazelrc".into())?,
            content: cpp_bazelrc_template().into(),
            existing: ExistingFilePolicy::Preserve,
        },
    ];
    Ok(MigrationPreview::External {
        commands,
        scaffolding,
    })
}

fn pre_validate_cmake(
    project_root: &Path,
    cmake_build_dir: Option<&str>,
    cmake_config: Option<&str>,
) -> Result<()> {
    let build_dir =
        cmake_build_dir.map_or_else(|| project_root.join(".rebaze/cmake-build"), PathBuf::from);
    std::fs::create_dir_all(&build_dir)?;

    let mut configure = Command::new("cmake");
    configure
        .arg("-S")
        .arg(project_root)
        .arg("-B")
        .arg(&build_dir);
    if let Some(config) = cmake_config {
        configure.arg(format!("-DCMAKE_BUILD_TYPE={config}"));
    }
    run_command(configure, "cmake configure")?;

    let mut build = Command::new("cmake");
    build.arg("--build").arg(&build_dir);
    if let Some(config) = cmake_config {
        build.arg("--config").arg(config);
    }
    run_command(build, "cmake build")?;

    Ok(())
}

fn post_validate_bazel(project_root: &Path) -> Result<()> {
    ensure_bazel_9()?;

    let mut cmd = Command::new("bazel");
    cmd.current_dir(project_root).arg("build").arg("//...");
    run_command(cmd, "bazel build")?;

    Ok(())
}

fn ensure_bazel_9() -> Result<()> {
    let output = Command::new("bazel")
        .arg("--version")
        .output()
        .context("Failed to run bazel --version")?;

    if !output.status.success() {
        anyhow::bail!("bazel --version failed");
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let version = stdout.trim();
    let major = parse_bazel_major(version).unwrap_or(0);
    if major != 9 {
        anyhow::bail!("Unsupported Bazel version: {version} (rebaze supports Bazel 9 only)");
    }

    Ok(())
}

fn parse_bazel_major(version: &str) -> Option<u32> {
    let token = version.split_whitespace().nth(1)?;
    let major = token.split('.').next()?;
    major.parse().ok()
}

fn run_command(mut cmd: Command, label: &str) -> Result<()> {
    tracing::info!("Running {label}");
    let status = cmd
        .status()
        .with_context(|| format!("Failed to run {label}"))?;
    if status.success() {
        Ok(())
    } else {
        anyhow::bail!("{label} failed with status {status}");
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn absent_file_api_can_fallback_but_present_invalid_reply_cannot() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(
            temp.path().join("CMakeLists.txt"),
            "project(example)\nadd_library(core STATIC core.c)",
        )
        .unwrap();
        let build = temp.path().join("build");
        let build_dir = build.to_str().unwrap();
        let (project, evidence) =
            parse_cmake_project(temp.path(), Some(build_dir), None, false).unwrap();
        assert_eq!(project.libraries[0].name, "core");
        assert!(matches!(evidence, report::SourceEvidence::CMakeSource));
        assert!(parse_cmake_project(temp.path(), Some(build_dir), None, true).is_err());

        let reply = build.join(".cmake/api/v1/reply");
        std::fs::create_dir_all(&reply).unwrap();
        std::fs::write(reply.join("index-test.json"), "{").unwrap();
        assert!(parse_cmake_project(temp.path(), Some(build_dir), None, false).is_err());
    }

    #[test]
    fn unsupported_file_api_preserves_outputs_even_with_fallback_enabled() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        std::fs::write(
            root.join("CMakeLists.txt"),
            "project(example)\nadd_library(core STATIC core.c)",
        )
        .unwrap();
        std::fs::write(root.join("BUILD.bazel"), "# existing output\n").unwrap();
        let build = root.join("build");
        let reply = build.join(".cmake/api/v1/reply");
        std::fs::create_dir_all(&reply).unwrap();
        let index = serde_json::json!({
            "cmake": {
                "version": {"major": 3, "minor": 27, "patch": 0, "suffix": "", "string": "3.27.0", "isDirty": false},
                "paths": {"cmake": "cmake", "ctest": "ctest", "cpack": "cpack", "root": "."},
                "generator": {"multiConfig": false, "name": "Ninja"}
            },
            "objects": [{"kind": "codemodel", "version": {"major": 2, "minor": 0}, "jsonFile": "model.json"}],
            "reply": {}
        });
        let model = serde_json::json!({
            "kind": "codemodel", "version": {"major": 2, "minor": 0},
            "paths": {"source": root, "build": build},
            "configurations": [{
                "name": "Debug", "projects": [{"name": "example", "directoryIndexes": []}],
                "directories": [],
                "targets": [{"name": "core", "id": "core", "directoryIndex": 0, "projectIndex": 0, "jsonFile": "target.json"}]
            }]
        });
        let target = serde_json::json!({
            "name": "core", "id": "core", "type": "STATIC_LIBRARY",
            "paths": {"source": ".", "build": "."},
            "sources": [{"path": "ChangeLog.md"}],
            "backtraceGraph": {"commands": [], "files": [], "nodes": []}
        });
        for (name, value) in [
            ("index-test.json", index),
            ("model.json", model),
            ("target.json", target),
        ] {
            std::fs::write(reply.join(name), serde_json::to_vec(&value).unwrap()).unwrap();
        }
        for file_api_only in [false, true] {
            let options = MigrateOptions {
                path: root.to_str().unwrap(),
                from: Some("cmake"),
                dry_run: false,
                cmake_build_dir: Some(build.to_str().unwrap()),
                cmake_config: Some("Debug"),
                cmake_file_api_only: file_api_only,
                build_generator: BuildGenerator::Native,
                bazelle_root: None,
                bazelle_bin: None,
                unsafe_mode: true,
                config: None,
            };
            let error = migrate(&options).unwrap_err();
            assert!(
                matches!(
                    error.downcast_ref::<rebaze_cmake_file_api::FileApiError>(),
                    Some(rebaze_cmake_file_api::FileApiError::UncompiledInput { target, path })
                        if target == "core" && path == Path::new("ChangeLog.md")
                ),
                "{error:#}"
            );
            assert_eq!(
                std::fs::read_to_string(root.join("BUILD.bazel")).unwrap(),
                "# existing output\n"
            );
            assert!(!root.join("MODULE.bazel").exists());
        }
    }

    #[test]
    fn unpinned_java_migration_requires_explicit_build_free_preparation() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(
            temp.path().join("settings.gradle"),
            "rootProject.name = 'example'",
        )
        .unwrap();
        std::fs::write(temp.path().join("build.gradle"), "plugins { id 'java' } repositories { mavenCentral() } dependencies { implementation 'junit:junit:4.13.2' }").unwrap();
        let mut options = MigrateOptions {
            path: temp.path().to_str().unwrap(),
            from: Some("gradle"),
            dry_run: false,
            cmake_build_dir: None,
            cmake_config: None,
            cmake_file_api_only: false,
            build_generator: BuildGenerator::Native,
            bazelle_root: None,
            bazelle_bin: None,
            unsafe_mode: false,
            config: None,
        };
        assert!(
            migrate(&options)
                .unwrap_err()
                .to_string()
                .contains("reviewed maven_install.json")
        );
        assert!(!temp.path().join("BUILD.bazel").exists());
        assert!(!temp.path().join("MODULE.bazel").exists());
        options.unsafe_mode = true;
        migrate(&options).unwrap();
        assert!(temp.path().join("MODULE.bazel").is_file());
        assert!(!temp.path().join("maven_install.json").exists());
    }

    #[test]
    fn native_cmake_dry_run_does_not_create_build_or_output_files() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(
            temp.path().join("CMakeLists.txt"),
            "project(example LANGUAGES CXX)\nadd_executable(example main.cpp)",
        )
        .unwrap();
        let options = MigrateOptions {
            path: temp.path().to_str().unwrap(),
            from: None,
            dry_run: true,
            cmake_build_dir: None,
            cmake_config: None,
            cmake_file_api_only: false,
            build_generator: BuildGenerator::Native,
            bazelle_root: None,
            bazelle_bin: None,
            unsafe_mode: false,
            config: None,
        };
        migrate(&options).unwrap();
        let files: Vec<_> = std::fs::read_dir(temp.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(files, ["CMakeLists.txt"]);
    }

    #[test]
    fn unsupported_source_fails_before_original_build_effects() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(
            temp.path().join("CMakeLists.txt"),
            "project(example C)\nif(OFF)\nadd_library(hidden hidden.c)\nendif()",
        )
        .unwrap();
        let options = MigrateOptions {
            path: temp.path().to_str().unwrap(),
            from: Some("cmake"),
            dry_run: false,
            cmake_build_dir: None,
            cmake_config: None,
            cmake_file_api_only: false,
            build_generator: BuildGenerator::Native,
            bazelle_root: None,
            bazelle_bin: None,
            unsafe_mode: false,
            config: None,
        };
        assert!(
            migrate(&options)
                .unwrap_err()
                .to_string()
                .contains("requires CMake evaluation")
        );
        let files: Vec<_> = std::fs::read_dir(temp.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(files, ["CMakeLists.txt"]);
    }

    #[test]
    fn generation_error_does_not_write_a_partial_migration() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(
            temp.path().join("CMakeLists.txt"),
            "project(example)\nadd_library(core STATIC core.c)",
        )
        .unwrap();
        let mut config = BazelConfig::default();
        config.build.rules.library_rule = "invalid-rule".into();
        let options = MigrateOptions {
            path: temp.path().to_str().unwrap(),
            from: Some("cmake"),
            dry_run: false,
            cmake_build_dir: None,
            cmake_config: None,
            cmake_file_api_only: false,
            build_generator: BuildGenerator::Native,
            bazelle_root: None,
            bazelle_bin: None,
            unsafe_mode: true,
            config: Some(Config {
                bazel: Some(config),
            }),
        };
        assert!(migrate(&options).is_err());
        let files: Vec<_> = std::fs::read_dir(temp.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(files, ["CMakeLists.txt"]);
    }

    #[test]
    fn bazelle_dry_run_does_not_build_the_generator() {
        let temp = tempfile::tempdir().unwrap();
        run_bazelle_generation(
            temp.path(),
            "example",
            &["cc".into()],
            Some("missing-workspace"),
            None,
            true,
        )
        .unwrap();
        assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 0);
    }

    #[test]
    fn discovered_invalid_config_is_an_error() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("rebaze.toml"), "[invalid").unwrap();
        assert!(Config::load_from_path_or_discover(None, temp.path()).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn path_candidates_require_execute_bits_without_running_a_program() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().unwrap();
        let candidate = temp.path().join("bazelle");
        std::fs::write(&candidate, "not executed").unwrap();
        std::fs::set_permissions(&candidate, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(!executable_candidate(&candidate));
        std::fs::set_permissions(&candidate, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(executable_candidate(&candidate));
        assert!(!executable_candidate(temp.path()));
    }

    #[test]
    fn existing_bazel_version_requires_major_nine() {
        let temp = tempfile::tempdir().unwrap();
        for version in ["", "90.0.0", "8.5.0", "9foo"] {
            std::fs::write(temp.path().join(".bazelversion"), version).unwrap();
            assert!(write_bazel_version(temp.path()).is_err(), "{version}");
        }
        std::fs::write(temp.path().join(".bazelversion"), "9.0.0\n").unwrap();
        write_bazel_version(temp.path()).unwrap();
    }
}
