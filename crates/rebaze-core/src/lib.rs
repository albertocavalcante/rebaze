//! Core migration logic for rebaze.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

pub mod config;
pub mod discovery;
pub mod model;

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

/// Migrate a project to Bazel.
pub fn migrate(options: &MigrateOptions<'_>) -> Result<()> {
    let project_root = std::fs::canonicalize(options.path)
        .with_context(|| format!("Failed to resolve project directory: {}", options.path))?;
    if !project_root.is_dir() {
        anyhow::bail!(
            "Project path is not a directory: {}",
            project_root.display()
        );
    }
    let path = project_root.as_path();
    let bazelle_available = options.build_generator != BuildGenerator::Native
        && bazelle_available(options.bazelle_bin, options.bazelle_root);

    let build_system = match options.from {
        Some(bs) => bs.to_string(),
        None => detect_build_system(path)?,
    };

    tracing::info!("Migrating from {build_system} to Bazel");

    match build_system.as_str() {
        "cmake" => {
            let generator = resolve_build_generator(
                build_system.as_str(),
                options.build_generator,
                bazelle_available,
            )?;
            let bazel_config = options
                .config
                .as_ref()
                .map_or_else(BazelConfig::default, Config::bazel_config);
            let profile = if generator == BuildGenerator::Native {
                rebaze_bazel::config::ConfigProfile::NativeCmake
            } else {
                rebaze_bazel::config::ConfigProfile::Bazelle
            };
            bazel_config.validate_for(profile)?;
            // Configured File API replies describe the validated build. Refresh
            // them before capture; source-only input can be prepared before IO.
            let original_validated =
                options.cmake_build_dir.is_some() && !options.unsafe_mode && !options.dry_run;
            if original_validated {
                pre_validate_cmake(path, options.cmake_build_dir, options.cmake_config)?;
            }
            let project = parse_cmake_project(
                path,
                options.cmake_build_dir,
                options.cmake_config,
                options.cmake_file_api_only,
            )?;
            match generator {
                BuildGenerator::Native => {
                    let generation = rebaze_bazel::plan_cmake(&project, &bazel_config)?;
                    if !original_validated && !options.unsafe_mode && !options.dry_run {
                        pre_validate_cmake(path, options.cmake_build_dir, options.cmake_config)?;
                    }
                    let observations = discovery::discover_packages(
                        generation.system_modules(),
                        &discovery::DiscoveryOptions::default(),
                    )?;
                    for module in generation.system_modules() {
                        use rebaze_bazel::discovery::ProbeOutcome;
                        match observations.outcome(module)? {
                            ProbeOutcome::Unavailable(reason) => tracing::warn!(
                                package = ?module.prefix, reason = %reason,
                                "Using unavailable-package fallback"
                            ),
                            ProbeOutcome::Available(info) if info.system.is_none() => {
                                tracing::warn!(
                                    package = ?module.prefix,
                                    "No installation prefix; system wrapper needs configuration"
                                );
                            }
                            // Failed probes are rejected by generation before writing outputs.
                            ProbeOutcome::Available(_) | ProbeOutcome::Failed(_) => {}
                        }
                    }
                    let bazel_files = generation.render(&observations)?;

                    tracing::info!("Generated {} Bazel files", bazel_files.len());
                    if options.dry_run {
                        print_files(&bazel_files);
                    } else {
                        rebaze_bazel::write_files(path, &bazel_files)?;
                    }
                }
                BuildGenerator::Bazelle => {
                    if !original_validated && !options.unsafe_mode && !options.dry_run {
                        pre_validate_cmake(path, options.cmake_build_dir, options.cmake_config)?;
                    }
                    let languages = bazelle_languages_for_cmake(&project);
                    run_bazelle_generation(
                        path,
                        &project.name,
                        &languages,
                        options.bazelle_root,
                        options.bazelle_bin,
                        options.dry_run,
                    )?;

                    if options.dry_run {
                        print_bazel_scaffolding();
                    } else {
                        write_bazel_version(path)?;
                        write_cpp_bazelrc(path)?;
                    }
                }
                BuildGenerator::Auto => unreachable!("auto resolved"),
            }
        }
        "gradle" => {
            let bazel_config = options
                .config
                .as_ref()
                .map_or_else(BazelConfig::default, Config::bazel_config);
            bazel_config.validate_for(rebaze_bazel::config::ConfigProfile::NativeGradle)?;
            let project = rebaze_gradle::parse(path).context("Failed to parse Gradle project")?;
            let generator = resolve_build_generator(
                build_system.as_str(),
                options.build_generator,
                bazelle_available,
            )?;

            match generator {
                BuildGenerator::Native => {
                    let bazel_files = rebaze_bazel::generate(&project)?;
                    if !options.dry_run
                        && !options.unsafe_mode
                        && project.maven_lock.is_none()
                        && project
                            .modules
                            .iter()
                            .flat_map(|module| &module.dependencies)
                            .any(|dep| matches!(dep, rebaze_gradle::Dependency::Maven(_)))
                    {
                        anyhow::bail!(
                            "Maven dependencies require a reviewed maven_install.json before build validation; use --dry-run to inspect, or --unsafe-mode to prepare files without executing builds, then explicitly resolve/review/pin @rebaze_maven before normal migration"
                        );
                    }

                    tracing::info!("Generated {} Bazel files", bazel_files.len());
                    if options.dry_run {
                        print_files(&bazel_files);
                    } else {
                        rebaze_bazel::write_files(path, &bazel_files)?;
                    }
                }
                BuildGenerator::Bazelle => {
                    anyhow::bail!(
                        "bazelle build generation is not supported for Gradle yet; use --build-generator native"
                    );
                }
                BuildGenerator::Auto => unreachable!("auto resolved"),
            }
        }
        _ => {
            anyhow::bail!("Unsupported build system: {build_system}");
        }
    }

    if !options.unsafe_mode && !options.dry_run {
        post_validate_bazel(path)?;
    }

    Ok(())
}

fn print_files(files: &std::collections::HashMap<String, String>) {
    let mut paths: Vec<_> = files.keys().collect();
    paths.sort_unstable();
    for file_path in paths {
        let content = &files[file_path];
        println!("--- {file_path} ---");
        println!("{content}");
    }
}

/// Validate generated Bazel files.
pub struct ValidateOptions<'a> {
    pub path: &'a str,
    pub unsafe_mode: bool,
}

pub fn validate(options: &ValidateOptions<'_>) -> Result<()> {
    let path = Path::new(options.path);

    if !path.join("MODULE.bazel").exists() && !path.join("WORKSPACE").exists() {
        anyhow::bail!("No Bazel workspace found at {}", path.display());
    }

    if options.unsafe_mode {
        tracing::info!("Skipping Bazel build validation (--unsafe-mode)");
        return Ok(());
    }

    post_validate_bazel(path)?;

    tracing::info!("Bazel build succeeded");
    Ok(())
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
) -> Result<rebaze_cmake::CMakeProject> {
    if let Some(build_dir) = cmake_build_dir {
        let options = rebaze_cmake_file_api::FileApiOptions {
            configuration: cmake_config.map(str::to_string),
        };
        match rebaze_cmake_file_api::parse_build_dir_with_options(Path::new(build_dir), &options) {
            Ok(project) => Ok(project),
            Err(err) => {
                if cmake_file_api_only {
                    return Err(err.into());
                }
                tracing::warn!(
                    "Failed to load cmake-file-api from {build_dir}: {err}. Falling back to parser."
                );
                rebaze_cmake::parse(path).context("Failed to parse CMake project")
            }
        }
    } else {
        if cmake_file_api_only {
            anyhow::bail!("--cmake-file-api-only requires --cmake-build-dir");
        }
        rebaze_cmake::parse(path).context("Failed to parse CMake project")
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

    Command::new("bazelle")
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success())
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
        println!("bazelle init --languages {languages_arg} --name {module_name}");
        println!("bazelle update --languages {languages_arg}");
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

    let content = r"# Generated by rebaze - C++ project settings

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
";

    std::fs::write(path, content)?;
    Ok(())
}

fn print_bazel_scaffolding() {
    println!("--- .bazelversion ---");
    println!("9.0.0");
    println!("--- .bazelrc ---");
    println!("# Generated by rebaze - C++ project settings");
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
