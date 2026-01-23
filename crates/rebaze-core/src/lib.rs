//! Core migration logic for rebaze.

use anyhow::{Context, Result};
use std::path::Path;

pub mod model;

/// Analyze a project and detect its build system.
pub fn analyze(path: &str) -> Result<String> {
    let path = Path::new(path);

    if !path.exists() {
        anyhow::bail!("Path does not exist: {}", path.display());
    }

    let mut detected = Vec::new();

    // Check for Gradle
    if path.join("build.gradle").exists() || path.join("build.gradle.kts").exists() {
        detected.push("gradle");
    }

    // Check for Maven
    if path.join("pom.xml").exists() {
        detected.push("maven");
    }

    // Check for CMake
    if path.join("CMakeLists.txt").exists() {
        detected.push("cmake");
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
pub fn migrate(path: &str, from: Option<&str>, dry_run: bool) -> Result<()> {
    let path = Path::new(path);

    let build_system = match from {
        Some(bs) => bs.to_string(),
        None => detect_build_system(path)?,
    };

    tracing::info!("Migrating from {} to Bazel", build_system);

    match build_system.as_str() {
        "gradle" => {
            let project = rebaze_gradle::parse(path).context("Failed to parse Gradle project")?;
            let bazel_files = rebaze_bazel::generate(&project);

            if dry_run {
                for (file_path, content) in &bazel_files {
                    println!("--- {file_path} ---");
                    println!("{content}");
                }
            } else {
                rebaze_bazel::write_files(path, &bazel_files)?;
            }
        }
        _ => {
            anyhow::bail!("Unsupported build system: {build_system}");
        }
    }

    Ok(())
}

/// Validate generated Bazel files.
pub fn validate(path: &str) -> Result<()> {
    let path = Path::new(path);

    if !path.join("MODULE.bazel").exists() && !path.join("WORKSPACE").exists() {
        anyhow::bail!("No Bazel workspace found at {}", path.display());
    }

    tracing::info!("Bazel files look valid");
    Ok(())
}

fn detect_build_system(path: &Path) -> Result<String> {
    if path.join("build.gradle").exists() || path.join("build.gradle.kts").exists() {
        return Ok("gradle".to_string());
    }
    if path.join("pom.xml").exists() {
        return Ok("maven".to_string());
    }
    if path.join("CMakeLists.txt").exists() {
        return Ok("cmake".to_string());
    }

    anyhow::bail!("Could not detect build system at {}", path.display())
}
