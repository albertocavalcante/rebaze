//! Explicit networked build oracles. Ordinary unit tests never run external tools.
#![cfg(unix)]
#![allow(clippy::unwrap_used)]

use std::path::Path;
use std::process::{Command, Output};

use crate::config::{KnownPackageInfo, SourceBuildSystem};
use crate::discovery::{PackageInfo, ProbeOutcome, SystemLibrary};
use crate::{DiscoverySnapshot, MigrationConfig, plan_cmake};

fn command(command: &mut Command) -> Output {
    // Fixture tools receive no unrelated parent credentials or build settings.
    let path = std::env::var_os("PATH").unwrap();
    let home = std::env::var_os("HOME").unwrap();
    command.env_clear().env("PATH", path).env("HOME", home);
    let output = command
        .output()
        .unwrap_or_else(|error| panic!("{command:?}: {error}"));
    assert!(
        output.status.success(),
        "{command:?}: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

pub fn build_and_run(root: &Path, files: &std::collections::HashMap<String, String>) {
    crate::write_files(root, files).unwrap();
    let cache = std::env::var_os("REBAZE_BAZEL_TEST_ROOT")
        .map_or_else(|| root.join("bazel-cache"), std::path::PathBuf::from);
    let output = command(
        Command::new("bazel")
            .current_dir(root)
            .args(["--batch", "--nosystem_rc", "--nohome_rc"])
            .arg(format!("--output_user_root={}", cache.display()))
            .args(["run", "//:app", "--noshow_progress", "--jobs=4"]),
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("rebaze dependency fixture: 42"));
}

fn project(package: &str) -> rebaze_cmake::CMakeProject {
    let parsed = rebaze_cmake::parse_source(&format!("project(strategy_fixture C)\npkg_check_modules(DEP REQUIRED {package})\nadd_executable(app main.c)\ntarget_link_libraries(app PRIVATE ${{DEP_LIBRARIES}})\n")).unwrap();
    rebaze_cmake::extract_project(&parsed, ".".into())
}

fn application(root: &Path) {
    std::fs::write(root.join("main.c"), "#include <stdio.h>\n#include <dep.h>\nint main(void) { int value = dep_value(); printf(\"rebaze dependency fixture: %d\\n\", value); return value != 42; }\n").unwrap();
}

#[test]
#[ignore = "explicit oracle: Bazel 9, CMake, Ninja, tar, shasum, C toolchain, and network"]
fn generated_source_dependency_builds_and_runs() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("dep-source");
    let root = temp.path().join("app");
    std::fs::create_dir(&source).unwrap();
    std::fs::create_dir(&root).unwrap();
    for (name, content) in [
        (
            "CMakeLists.txt",
            include_str!("../tests/fixtures/managed_dependency/CMakeLists.txt"),
        ),
        (
            "dep.c",
            include_str!("../tests/fixtures/managed_dependency/dep.c"),
        ),
        (
            "dep.h",
            include_str!("../tests/fixtures/managed_dependency/dep.h"),
        ),
    ] {
        std::fs::write(source.join(name), content).unwrap();
    }
    let archive = temp.path().join("dep.tar.gz");
    command(
        Command::new("tar")
            .arg("-czf")
            .arg(&archive)
            .arg("-C")
            .arg(&source)
            .arg("."),
    );
    let digest = command(Command::new("shasum").args(["-a", "256"]).arg(&archive));
    let digest = String::from_utf8(digest.stdout)
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap()
        .to_owned();
    let mut config = MigrationConfig::default();
    config.strategy.default = "source".into();
    config.mappings.known_packages.insert(
        "dep".into(),
        KnownPackageInfo {
            version: "1.0.0".into(),
            url: format!("file://{}", archive.display()),
            sha256: digest,
            strip_prefix: String::new(),
            build_system: SourceBuildSystem::Cmake,
            pkg_config_packages: vec!["dep".into()],
            out_static_libs: vec!["libdep.a".into()],
            out_shared_libs: vec![],
            deps: vec![],
            build_options: vec!["-DFIXTURE_VALUE=42".into()],
        },
    );
    let project = project("dep>=1.0");
    let prepared = plan_cmake(&project, &config).unwrap();
    assert!(prepared.system_modules().is_empty());
    application(&root);
    build_and_run(
        &root,
        &prepared.render(&DiscoverySnapshot::default()).unwrap(),
    );
}

#[test]
#[ignore = "explicit oracle: Bazel 9, C compiler, ar, and network"]
fn generated_system_dependency_preserves_paths_defines_and_link_flags() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("app");
    let prefix = temp.path().join("system prefix with spaces ' and $");
    // This include directory is deliberately outside the installation prefix.
    let include = temp.path().join("external headers");
    std::fs::create_dir(&root).unwrap();
    std::fs::create_dir(&prefix).unwrap();
    std::fs::create_dir(&include).unwrap();
    std::fs::write(
        include.join("dep.h"),
        include_str!("../tests/fixtures/managed_dependency/dep.h"),
    )
    .unwrap();
    std::fs::write(
        prefix.join("dep.c"),
        include_str!("../tests/fixtures/managed_dependency/dep.c"),
    )
    .unwrap();
    command(
        Command::new("cc")
            .arg("-c")
            .arg(prefix.join("dep.c"))
            .arg("-I")
            .arg(&include)
            .arg("-DFIXTURE_VALUE=42")
            .arg("-o")
            .arg(prefix.join("dep.o")),
    );
    command(
        Command::new("ar")
            .arg("rcs")
            .arg(prefix.join("libdep.a"))
            .arg(prefix.join("dep.o")),
    );
    let project = project("dep");
    let config = MigrationConfig::default();
    let prepared = plan_cmake(&project, &config).unwrap();
    let mut snapshot = DiscoverySnapshot::default();
    snapshot
        .record(
            &prepared.system_modules()[0],
            ProbeOutcome::Available(PackageInfo {
                cflags: vec![
                    format!("-I{}", include.display()),
                    "-DREBAZE_SYSTEM_FLAG=42".into(),
                ],
                linkopts: vec![
                    format!("-L{}", prefix.display()),
                    "-ldep".into(),
                    prefix.join("libdep.a").display().to_string(),
                    "-pthread".into(),
                ],
                system: Some(SystemLibrary {
                    prefix: prefix.display().to_string(),
                    include_dirs: vec![include.display().to_string()],
                    lib_dirs: vec![],
                    libs: vec![],
                }),
            }),
        )
        .unwrap();
    application(&root);
    let main = std::fs::read_to_string(root.join("main.c")).unwrap();
    std::fs::write(
        root.join("main.c"),
        format!("#if REBAZE_SYSTEM_FLAG != 42\n#error missing propagated define\n#endif\n{main}"),
    )
    .unwrap();
    build_and_run(&root, &prepared.render(&snapshot).unwrap());
}

#[test]
#[ignore = "explicit oracle: Bazel 9, C toolchain, and network"]
fn generated_bcr_dependency_builds_and_runs() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let project = project("zlib>=1.2");
    let mut config = MigrationConfig::default();
    config.strategy.default = "bcr".into();
    config
        .strategy
        .bcr
        .version_pins
        .insert("zlib".into(), "1.3.1.bcr.5".into());
    let prepared = plan_cmake(&project, &config).unwrap();
    assert!(prepared.system_modules().is_empty());
    std::fs::write(root.join("main.c"), "#include <stdio.h>\n#include <zlib.h>\nint main(void) { if (compressBound(1) == 0) return 1; puts(\"rebaze dependency fixture: 42\"); return 0; }\n").unwrap();
    build_and_run(
        root,
        &prepared.render(&DiscoverySnapshot::default()).unwrap(),
    );
}
