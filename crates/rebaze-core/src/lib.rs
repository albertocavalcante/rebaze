//! Core migration logic for rebaze.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

pub mod model;

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
pub fn migrate(options: MigrateOptions<'_>) -> Result<()> {
    let path = Path::new(options.path);
    let bazelle_available = bazelle_available(options.bazelle_bin, options.bazelle_root);

    let build_system = match options.from {
        Some(bs) => bs.to_string(),
        None => detect_build_system(path)?,
    };

    tracing::info!("Migrating from {build_system} to Bazel");

    match build_system.as_str() {
        "cmake" => {
            if !options.unsafe_mode {
                pre_validate_cmake(path, options.cmake_build_dir, options.cmake_config)?;
            }

            let project = parse_cmake_project(
                path,
                options.cmake_build_dir,
                options.cmake_config,
                options.cmake_file_api_only,
            )?;
            let generator = resolve_build_generator(
                build_system.as_str(),
                options.build_generator,
                bazelle_available,
            )?;

            match generator {
                BuildGenerator::Native => {
                    let bazel_files = rebaze_bazel::generate_from_cmake(&project);

                    if options.dry_run {
                        print_files(&bazel_files);
                    } else {
                        rebaze_bazel::write_files(path, &bazel_files)?;
                    }
                }
                BuildGenerator::Bazelle => {
                    run_bazelle_generation(
                        path,
                        &project.name,
                        bazelle_languages_for_cmake(&project),
                        options.bazelle_root,
                        options.bazelle_bin,
                        options.dry_run,
                    )?;

                    if !options.dry_run {
                        write_bazel_version(path)?;
                        write_cpp_bazelrc(path)?;
                    } else {
                        print_bazel_scaffolding();
                    }
                }
                BuildGenerator::Auto => unreachable!("auto resolved"),
            }
        }
        "gradle" => {
            let project = rebaze_gradle::parse(path).context("Failed to parse Gradle project")?;
            let generator = resolve_build_generator(
                build_system.as_str(),
                options.build_generator,
                bazelle_available,
            )?;

            match generator {
                BuildGenerator::Native => {
                    let bazel_files = rebaze_bazel::generate(&project);

                    if options.dry_run {
                        print_files(&bazel_files);
                    } else {
                        rebaze_bazel::write_files(path, &bazel_files)?;
                    }
                }
                BuildGenerator::Bazelle => {
                    anyhow::bail!("bazelle build generation is not supported for Gradle yet; use --build-generator native");
                }
                BuildGenerator::Auto => unreachable!("auto resolved"),
            }
        }
        _ => {
            anyhow::bail!("Unsupported build system: {build_system}");
        }
    }

    if !options.unsafe_mode {
        post_validate_bazel(path)?;
    }

    Ok(())
}

fn print_files(files: &std::collections::HashMap<String, String>) {
    for (file_path, content) in files {
        println!("--- {file_path} ---");
        println!("{content}");
    }
}

/// Validate generated Bazel files.
pub struct ValidateOptions<'a> {
    pub path: &'a str,
    pub unsafe_mode: bool,
}

pub fn validate(options: ValidateOptions<'_>) -> Result<()> {
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
        match rebaze_cmake_file_api::parse_build_dir_with_options(Path::new(build_dir), &options)
        {
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
        .map(|output| output.status.success())
        .unwrap_or(false)
}

fn run_bazelle_generation(
    project_root: &Path,
    module_name: &str,
    languages: Vec<String>,
    bazelle_root: Option<&str>,
    bazelle_bin: Option<&str>,
    dry_run: bool,
) -> Result<()> {
    let bazelle = ensure_bazelle_binary(bazelle_root, bazelle_bin)?;
    let languages_arg = languages.join(",");

    let mut init_cmd = Command::new(&bazelle);
    init_cmd.current_dir(project_root);
    init_cmd.arg("init");
    init_cmd.arg("--languages").arg(&languages_arg);
    init_cmd.arg("--name").arg(module_name);
    if dry_run {
        init_cmd.arg("--dry-run");
    }
    run_command(init_cmd, "bazelle init")?;

    let mut update_cmd = Command::new(&bazelle);
    update_cmd.current_dir(project_root);
    update_cmd.arg("update");
    update_cmd.arg("--languages").arg(&languages_arg);
    if dry_run {
        update_cmd.arg("--check");
    }
    run_command(update_cmd, "bazelle update")?;

    Ok(())
}

fn ensure_bazelle_binary(
    bazelle_root: Option<&str>,
    bazelle_bin: Option<&str>,
) -> Result<PathBuf> {
    if let Some(bin) = bazelle_bin {
        let path = PathBuf::from(bin);
        if path.is_file() {
            return Ok(path);
        }
        anyhow::bail!("bazelle binary not found at {}", path.display());
    }

    if let Some(root) = bazelle_root {
        let root = PathBuf::from(root);
        if !root.exists() {
            anyhow::bail!("bazelle root not found at {}", root.display());
        }

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

fn bazelle_languages_for_cmake(project: &rebaze_cmake::CMakeProject) -> Vec<String> {
    let mut languages = Vec::new();
    let mut has_cpp = false;
    let mut has_c = false;

    for lang in &project.languages {
        match lang.as_str() {
            "C" => has_c = true,
            "CXX" | "C++" => has_cpp = true,
            _ => {}
        }
    }

    if has_c || has_cpp {
        languages.push("cc".to_string());
    }

    if languages.is_empty() {
        languages.push("cc".to_string());
    }

    languages
}

fn write_bazel_version(root: &Path) -> Result<()> {
    let path = root.join(".bazelversion");
    let desired = "9.0.0\n";

    if path.exists() {
        let content = std::fs::read_to_string(&path)?;
        if let Some(version) = content.lines().next() {
            if !version.trim().starts_with('9') {
                anyhow::bail!(
                    "Unsupported Bazel version in {}: {} (rebaze supports Bazel 9 only)",
                    path.display(),
                    version.trim()
                );
            }
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
    let build_dir = cmake_build_dir
        .map(PathBuf::from)
        .unwrap_or_else(|| project_root.join(".rebaze/cmake-build"));
    std::fs::create_dir_all(&build_dir)?;

    let mut configure = Command::new("cmake");
    configure.arg("-S").arg(project_root).arg("-B").arg(&build_dir);
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
    let status = cmd.status().with_context(|| format!("Failed to run {label}"))?;
    if status.success() {
        Ok(())
    } else {
        anyhow::bail!("{label} failed with status {status}");
    }
}
