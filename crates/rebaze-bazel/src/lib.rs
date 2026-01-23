//! Bazel file generator for rebaze.

use anyhow::Result;
use std::collections::HashMap;
use std::path::Path;

// Re-export for use by other crates
pub use crate::generator::BazelFile;

mod generator;

/// Generate Bazel files from a parsed Gradle project.
#[must_use]
pub fn generate(project: &rebaze_gradle::GradleProject) -> HashMap<String, String> {
    let mut files = HashMap::new();

    // Generate MODULE.bazel
    let module_bazel = generator::generate_module_bazel(project);
    files.insert("MODULE.bazel".to_string(), module_bazel);

    // Generate root BUILD.bazel
    let root_build = generator::generate_root_build(project);
    files.insert("BUILD.bazel".to_string(), root_build);

    // Generate BUILD.bazel for each subproject
    for subproject in &project.subprojects {
        let subproject_path = subproject.replace(':', "/");
        let build_path = format!("{subproject_path}/BUILD.bazel");
        let build_content = generator::generate_subproject_build(project, subproject);
        files.insert(build_path, build_content);
    }

    // Generate .bazelversion
    files.insert(".bazelversion".to_string(), "9.0.0\n".to_string());

    tracing::info!("Generated {} Bazel files", files.len());

    files
}

/// Write generated Bazel files to disk.
pub fn write_files<S: std::hash::BuildHasher>(
    root: &Path,
    files: &HashMap<String, String, S>,
) -> Result<()> {
    for (path, content) in files {
        let full_path = root.join(path);

        if let Some(parent) = full_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        std::fs::write(&full_path, content)?;
        tracing::info!("Wrote {}", full_path.display());
    }

    Ok(())
}
