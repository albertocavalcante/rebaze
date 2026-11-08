//! Migration orchestration. Every effect is recorded separately from lowering.

use std::path::Path;

use anyhow::{Context, Result};
use rebaze_bazel::GenerationOutput;

use crate::report::{
    BuildClaim, ClaimResult, DiscoveryReport, MigrationFailure, MigrationPreview, MigrationReport,
    SkipReason, SourceEvidence, Stage, StageOutcome, StageRecord,
};
use crate::{BazelConfig, BuildGenerator, Config, MigrateOptions, discovery};

/// Compatibility entry point. Presentation belongs to the caller/CLI.
pub fn migrate(options: &MigrateOptions<'_>) -> Result<()> {
    migrate_with_report(options)
        .map(|_| ())
        .map_err(|failure| failure.source)
}

/// Execute selected stages and return actual evidence, including on failure.
pub fn migrate_with_report(
    options: &MigrateOptions<'_>,
) -> std::result::Result<MigrationReport, MigrationFailure> {
    let mut run = MigrationRun::default();
    if let Err(source) = execute(options, &mut run) {
        return Err(MigrationFailure {
            report: Box::new(run.report),
            source,
        });
    }
    Ok(run.report)
}

#[derive(Default)]
struct MigrationRun {
    report: MigrationReport,
}

impl MigrationRun {
    fn stage<T>(&mut self, stage: Stage, action: impl FnOnce() -> Result<T>) -> Result<T> {
        match action() {
            Ok(value) => {
                self.report.stages.push(StageRecord {
                    stage,
                    outcome: StageOutcome::Passed,
                });
                Ok(value)
            }
            Err(error) => {
                let diagnostic = error
                    .chain()
                    .find_map(|cause| cause.downcast_ref::<rebaze_bazel::GenerationError>())
                    .map(rebaze_bazel::GenerationError::diagnostic);
                self.report.stages.push(StageRecord {
                    stage,
                    outcome: StageOutcome::Failed {
                        message: format!("{error:#}"),
                        diagnostic,
                    },
                });
                Err(error)
            }
        }
    }

    fn skip(&mut self, stage: Stage, reason: SkipReason) {
        self.report.stages.push(StageRecord {
            stage,
            outcome: StageOutcome::Skipped(reason),
        });
    }

    fn build(&mut self, claim: BuildClaim, action: impl FnOnce() -> Result<()>) -> Result<()> {
        let stage = match claim {
            BuildClaim::OriginalBuild => Stage::OriginalBuild,
            BuildClaim::GeneratedBuild => Stage::GeneratedBuild,
        };
        let result = self.stage(stage, action);
        self.report.claims.record(
            claim,
            if result.is_ok() {
                ClaimResult::Passed
            } else {
                ClaimResult::Failed
            },
        );
        result
    }

    fn output(&mut self, root: &Path, output: GenerationOutput, preview: bool) -> Result<()> {
        self.report.outputs = output.bundle.iter().map(|(path, _)| path.clone()).collect();
        self.report.lowering = Some(output.report);
        if preview {
            self.skip(Stage::Application, SkipReason::Preview);
            self.report.preview = Some(MigrationPreview::Files(output.bundle));
            Ok(())
        } else {
            self.stage(Stage::Application, || {
                rebaze_bazel::write_bundle(root, &output.bundle)
            })
        }
    }
}

const fn validation_skip(options: &MigrateOptions<'_>) -> Option<SkipReason> {
    if options.dry_run {
        Some(SkipReason::Preview)
    } else if options.unsafe_mode {
        Some(SkipReason::ValidationDisabled)
    } else {
        None
    }
}

fn execute(options: &MigrateOptions<'_>, run: &mut MigrationRun) -> Result<()> {
    let root = run.stage(Stage::WorkspaceInspection, || {
        let root = std::fs::canonicalize(options.path)
            .with_context(|| format!("Failed to resolve project directory: {}", options.path))?;
        anyhow::ensure!(
            root.is_dir(),
            "Project path is not a directory: {}",
            root.display()
        );
        Ok(root)
    })?;
    run.report.root = Some(root.clone());
    let system = run.stage(Stage::BuildSystemSelection, || {
        options.from.map_or_else(
            || super::detect_build_system(&root),
            |name| Ok(name.to_string()),
        )
    })?;
    let config = options
        .config
        .as_ref()
        .map_or_else(BazelConfig::default, Config::bazel_config);
    match system.as_str() {
        "cmake" => cmake(options, &root, &config, run)?,
        "gradle" => gradle(options, &root, &config, run)?,
        _ => run.stage(Stage::Preparation, || {
            anyhow::bail!("Unsupported build system: {system}")
        })?,
    }
    if let Some(reason) = validation_skip(options) {
        run.skip(Stage::GeneratedBuild, reason);
    } else {
        run.build(BuildClaim::GeneratedBuild, || {
            super::post_validate_bazel(&root)
        })?;
    }
    Ok(())
}

fn cmake(
    options: &MigrateOptions<'_>,
    root: &Path,
    config: &BazelConfig,
    run: &mut MigrationRun,
) -> Result<()> {
    run.report.claims.require(BuildClaim::OriginalBuild);
    let generator = run.stage(Stage::GeneratorSelection, || {
        let present = options.build_generator != BuildGenerator::Native
            && super::bazelle_available(options.bazelle_bin, options.bazelle_root);
        super::resolve_build_generator("cmake", options.build_generator, present)
    })?;
    run.report.generator = Some(generator);
    let profile = if generator == BuildGenerator::Native {
        rebaze_bazel::config::ConfigProfile::NativeCmake
    } else {
        rebaze_bazel::config::ConfigProfile::Bazelle
    };
    run.stage(Stage::Settings, || {
        Ok(config
            .validate_for(profile)
            .map_err(rebaze_bazel::GenerationError::from)?)
    })?;
    // Explicit configured evidence must follow its original build refresh.
    let refreshed = options.cmake_build_dir.is_some() && validation_skip(options).is_none();
    if refreshed {
        run.build(BuildClaim::OriginalBuild, || {
            super::pre_validate_cmake(root, options.cmake_build_dir, options.cmake_config)
        })?;
    }
    let (project, evidence) = run.stage(Stage::Capture, || {
        super::parse_cmake_project(
            root,
            options.cmake_build_dir,
            options.cmake_config,
            options.cmake_file_api_only,
        )
    })?;
    run.report.source = Some(evidence);
    match generator {
        BuildGenerator::Native => {
            let generation = run.stage(Stage::Preparation, || {
                Ok(rebaze_bazel::plan_cmake(&project, config)?)
            })?;
            if !refreshed {
                original_build(options, root, run)?;
            }
            let policy = discovery::DiscoveryOptions::default();
            run.report.discovery = Some(DiscoveryReport {
                requests: generation.system_modules().to_vec(),
                command_timeout: policy.command_timeout,
                max_output_bytes: policy.max_output_bytes,
                observations: None,
            });
            let observations = run.stage(Stage::HostDiscovery, || {
                let observations = discovery::discover_packages(generation.system_modules(), &policy)?;
                for module in generation.system_modules() {
                    use rebaze_bazel::discovery::ProbeOutcome;
                    match observations.outcome(module)? {
                        ProbeOutcome::Unavailable(reason) => tracing::warn!(package = ?module.prefix, reason = %reason, "Using unavailable-package fallback"),
                        ProbeOutcome::Available(info) if info.system.is_none() => tracing::warn!(package = ?module.prefix, "No installation prefix; system wrapper needs configuration"),
                        ProbeOutcome::Available(_) | ProbeOutcome::Failed(_) => {}
                    }
                }
                Ok(observations)
            })?;
            if let Some(discovery) = &mut run.report.discovery {
                discovery.observations = Some(observations.clone());
            }
            let output = run.stage(Stage::Rendering, || {
                Ok(generation.render_bundle(&observations)?)
            })?;
            run.output(root, output, options.dry_run)?;
        }
        BuildGenerator::Bazelle => {
            if !refreshed {
                original_build(options, root, run)?;
            }
            let languages = super::bazelle_languages_for_cmake(&project);
            if options.dry_run {
                let preview = run.stage(Stage::Preparation, || {
                    super::bazelle_preview(
                        root,
                        &project.name,
                        &languages,
                        options.bazelle_root,
                        options.bazelle_bin,
                    )
                })?;
                run.report.preview = Some(preview);
                run.skip(Stage::ExternalGeneration, SkipReason::Preview);
                run.skip(Stage::Application, SkipReason::Preview);
            } else {
                run.stage(Stage::ExternalGeneration, || {
                    super::run_bazelle_generation(
                        root,
                        &project.name,
                        &languages,
                        options.bazelle_root,
                        options.bazelle_bin,
                        false,
                    )
                })?;
                run.stage(Stage::Application, || {
                    super::write_bazel_version(root)?;
                    super::write_cpp_bazelrc(root)
                })?;
            }
        }
        BuildGenerator::Auto => unreachable!("auto resolved"),
    }
    Ok(())
}

fn original_build(options: &MigrateOptions<'_>, root: &Path, run: &mut MigrationRun) -> Result<()> {
    if let Some(reason) = validation_skip(options) {
        run.skip(Stage::OriginalBuild, reason);
    } else {
        run.build(BuildClaim::OriginalBuild, || {
            super::pre_validate_cmake(root, options.cmake_build_dir, options.cmake_config)
        })?;
    }
    Ok(())
}

fn gradle(
    options: &MigrateOptions<'_>,
    root: &Path,
    config: &BazelConfig,
    run: &mut MigrationRun,
) -> Result<()> {
    run.stage(Stage::Settings, || {
        Ok(config
            .validate_for(rebaze_bazel::config::ConfigProfile::NativeGradle)
            .map_err(rebaze_bazel::GenerationError::from)?)
    })?;
    run.stage(Stage::GeneratorSelection, || {
        anyhow::ensure!(
            options.build_generator != BuildGenerator::Bazelle,
            "bazelle build generation is not supported for Gradle yet; use --build-generator native"
        );
        Ok(())
    })?;
    run.report.generator = Some(BuildGenerator::Native);
    let project = run.stage(Stage::Capture, || {
        rebaze_gradle::parse(root).context("Failed to parse Gradle project")
    })?;
    run.report.source = Some(SourceEvidence::GradleSource);
    run.stage(Stage::Preparation, || {
        if validation_skip(options).is_none() && project.maven_lock.is_none() && project.modules.iter().flat_map(|module| &module.dependencies).any(|dep| matches!(dep, rebaze_gradle::Dependency::Maven(_))) {
            anyhow::bail!("Maven dependencies require a reviewed maven_install.json before build validation; use --dry-run to inspect, or --unsafe-mode to prepare files without executing builds, then explicitly resolve/review/pin @rebaze_maven before normal migration");
        }
        Ok(())
    })?;
    let output = run.stage(Stage::Rendering, || {
        Ok(rebaze_bazel::generate_bundle(&project)?)
    })?;
    run.output(root, output, options.dry_run)
}

/// Compatibility validation entry point; reports remain available to new callers.
pub fn validate(options: &crate::ValidateOptions<'_>) -> Result<()> {
    validate_with_report(options)
        .map(|_| ())
        .map_err(|failure| failure.source)
}

pub fn validate_with_report(
    options: &crate::ValidateOptions<'_>,
) -> std::result::Result<MigrationReport, MigrationFailure> {
    let mut run = MigrationRun::default();
    let result = (|| {
        let root = run.stage(Stage::WorkspaceInspection, || {
            let root =
                std::fs::canonicalize(options.path).context("Failed to resolve Bazel workspace")?;
            anyhow::ensure!(
                root.join("MODULE.bazel").is_file() || root.join("WORKSPACE").is_file(),
                "No Bazel workspace found at {}",
                root.display()
            );
            Ok(root)
        })?;
        run.report.root = Some(root.clone());
        if options.unsafe_mode {
            run.skip(Stage::GeneratedBuild, SkipReason::ValidationDisabled);
        } else {
            run.build(BuildClaim::GeneratedBuild, || {
                super::post_validate_bazel(&root)
            })?;
        }
        Ok(())
    })();
    if let Err(source) = result {
        return Err(MigrationFailure {
            report: Box::new(run.report),
            source,
        });
    }
    Ok(run.report)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::report::{MigrationPreview, SourceEvidence};

    fn input() -> tempfile::TempDir {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(
            temp.path().join("CMakeLists.txt"),
            "project(example C)\nadd_executable(app main.c)",
        )
        .unwrap();
        temp
    }

    fn options(path: &Path, dry_run: bool, unsafe_mode: bool) -> MigrateOptions<'_> {
        MigrateOptions {
            path: path.to_str().unwrap(),
            from: Some("cmake"),
            dry_run,
            cmake_build_dir: None,
            cmake_config: None,
            cmake_file_api_only: false,
            build_generator: BuildGenerator::Native,
            bazelle_root: None,
            bazelle_bin: None,
            unsafe_mode,
            config: None,
        }
    }

    #[test]
    fn preview_returns_stable_intent_and_unknown_build_claims() {
        let temp = input();
        let report = migrate_with_report(&options(temp.path(), true, false)).unwrap();
        let again = migrate_with_report(&options(temp.path(), true, false)).unwrap();
        assert_eq!(report.source, Some(SourceEvidence::CMakeSource));
        assert_eq!(report.generator, Some(BuildGenerator::Native));
        assert_eq!(report.claims.aggregate(), ClaimResult::Unknown);
        assert_eq!(
            report.claims.result(BuildClaim::OriginalBuild),
            ClaimResult::Unknown
        );
        assert_eq!(
            report.claims.result(BuildClaim::GeneratedBuild),
            ClaimResult::Unknown
        );
        assert!(matches!(report.preview, Some(MigrationPreview::Files(_))));
        assert_eq!(report.outputs.len(), 4);
        assert_eq!(report.stages, again.stages);
        assert_eq!(report.claims, again.claims);
        assert_eq!(report.lowering, again.lowering);
        assert_eq!(report.outputs, again.outputs);
        assert_eq!(report.preview, again.preview);
        assert!(!temp.path().join(".rebaze").exists());
        assert!(!temp.path().join("BUILD.bazel").exists());
    }

    #[test]
    fn failed_preparation_keeps_completed_stages_and_precise_diagnostic() {
        let temp = input();
        std::fs::write(
            temp.path().join("CMakeLists.txt"),
            "project(example C)\nif(OFF)\nadd_executable(hidden hidden.c)\nendif()",
        )
        .unwrap();
        let failure = migrate_with_report(&options(temp.path(), false, false)).unwrap_err();
        let report = failure.report;
        assert_eq!(report.claims.aggregate(), ClaimResult::Unknown);
        assert_eq!(report.outputs, []);
        assert!(report.preview.is_none());
        assert!(
            matches!(report.stages.last(), Some(StageRecord { stage: Stage::Preparation, outcome: StageOutcome::Failed { diagnostic: Some(diagnostic), .. } }) if diagnostic.code == rebaze_bazel::DiagnosticCode::SourceEvaluation)
        );
        assert!(!temp.path().join(".rebaze").exists());
        assert!(!temp.path().join("BUILD.bazel").exists());
    }

    #[test]
    fn source_budget_rejections_preserve_existing_output_before_application() {
        for (source, reason) in [
            (format!("#{}", "x".repeat(1024 * 1024)), "file bytes"),
            (
                format!("message({}leaf{})", "(".repeat(33), ")".repeat(33)),
                "nesting budget",
            ),
        ] {
            let temp = input();
            std::fs::write(temp.path().join("CMakeLists.txt"), source).unwrap();
            let destination = temp.path().join("BUILD.bazel");
            std::fs::write(&destination, "user output").unwrap();
            let failure = migrate_with_report(&options(temp.path(), false, true)).unwrap_err();
            assert!(format!("{:#}", failure.source).contains(reason));
            assert!(matches!(
                failure.report.stages.last(),
                Some(StageRecord {
                    stage: Stage::Capture,
                    outcome: StageOutcome::Failed { .. }
                })
            ));
            assert_eq!(failure.report.outputs, []);
            assert_eq!(std::fs::read_to_string(destination).unwrap(), "user output");
            assert!(!temp.path().join(".rebaze-apply").exists());
            assert!(!temp.path().join("MODULE.bazel").exists());
        }
    }

    #[cfg(unix)]
    #[test]
    fn escaped_cmake_child_preserves_existing_output() {
        let temp = input();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(
            outside.path().join("CMakeLists.txt"),
            "add_library(escaped escaped.c)",
        )
        .unwrap();
        std::os::unix::fs::symlink(outside.path(), temp.path().join("child")).unwrap();
        std::fs::write(
            temp.path().join("CMakeLists.txt"),
            "project(example)\nadd_subdirectory(child)",
        )
        .unwrap();
        let output = temp.path().join("BUILD.bazel");
        std::fs::write(&output, "user output").unwrap();
        let failure = migrate_with_report(&options(temp.path(), false, true)).unwrap_err();
        assert!(matches!(
            failure.report.stages.last(),
            Some(StageRecord {
                stage: Stage::Capture,
                outcome: StageOutcome::Failed { .. }
            })
        ));
        assert_eq!(failure.report.outputs, []);
        assert_eq!(std::fs::read_to_string(output).unwrap(), "user output");
        assert!(!temp.path().join("MODULE.bazel").exists());
        assert!(!temp.path().join(".rebaze-apply").exists());
    }

    #[test]
    fn gradle_input_budgets_fail_capture_without_replacing_output() {
        for (wide, reason) in [(false, "file bytes"), (true, "directory entries")] {
            let root = tempfile::tempdir().unwrap();
            std::fs::write(root.path().join("build.gradle"), "plugins { id 'java' }").unwrap();
            if wide {
                std::fs::write(
                    root.path().join("settings.gradle"),
                    "rootProject.name = 'example'",
                )
                .unwrap();
                let sources = root.path().join("src/main/java");
                std::fs::create_dir_all(&sources).unwrap();
                for index in 0..8193 {
                    std::fs::write(sources.join(format!("ignored-{index}.bin")), "").unwrap();
                }
            } else {
                std::fs::write(
                    root.path().join("settings.gradle"),
                    format!("//{}", "x".repeat(1024 * 1024)),
                )
                .unwrap();
            }
            let output = root.path().join("BUILD.bazel");
            std::fs::write(&output, "user output").unwrap();
            let mut options = options(root.path(), false, true);
            options.from = Some("gradle");
            let failure = migrate_with_report(&options).unwrap_err();
            assert!(format!("{:#}", failure.source).contains(reason));
            assert!(matches!(
                failure.report.stages.last(),
                Some(StageRecord {
                    stage: Stage::Capture,
                    outcome: StageOutcome::Failed { .. }
                })
            ));
            assert_eq!(failure.report.outputs, []);
            assert_eq!(std::fs::read_to_string(output).unwrap(), "user output");
            assert!(!root.path().join("MODULE.bazel").exists());
            assert!(!root.path().join(".rebaze-apply").exists());
        }
    }

    #[test]
    fn config_failure_records_available_field_without_running_builds() {
        let temp = input();
        let mut options = options(temp.path(), false, false);
        let mut config = BazelConfig::default();
        config.filters.excluded_define_patterns = vec!["CUSTOM".into()];
        options.config = Some(Config {
            bazel: Some(config),
        });
        let failure = migrate_with_report(&options).unwrap_err();
        assert!(
            matches!(failure.report.stages.last(), Some(StageRecord { stage: Stage::Settings, outcome: StageOutcome::Failed { diagnostic: Some(diagnostic), .. } }) if diagnostic.origin == rebaze_bazel::DiagnosticOrigin::ConfigurationField("filters.excluded_define_patterns".into()))
        );
        assert_eq!(failure.report.claims.aggregate(), ClaimResult::Unknown);
        assert!(!temp.path().join(".rebaze").exists());
    }

    #[test]
    fn gradle_settings_failure_retains_its_diagnostic_category() {
        let temp = input();
        let mut options = options(temp.path(), false, false);
        options.from = Some("gradle");
        let mut config = BazelConfig::default();
        config.build.default_c_standard = "17".into();
        options.config = Some(Config {
            bazel: Some(config),
        });
        let failure = migrate_with_report(&options).unwrap_err();
        assert!(
            matches!(failure.report.stages.last(), Some(StageRecord { stage: Stage::Settings, outcome: StageOutcome::Failed { diagnostic: Some(diagnostic), .. } }) if diagnostic.code == rebaze_bazel::DiagnosticCode::Configuration)
        );
        assert_eq!(failure.report.claims.aggregate(), ClaimResult::Unknown);
    }

    #[test]
    fn applied_files_do_not_imply_skipped_builds_passed() {
        let temp = input();
        let report = migrate_with_report(&options(temp.path(), false, true)).unwrap();
        assert_eq!(report.claims.aggregate(), ClaimResult::Unknown);
        assert!(report.preview.is_none());
        assert!(report.stages.contains(&StageRecord {
            stage: Stage::Application,
            outcome: StageOutcome::Passed
        }));
        assert!(temp.path().join("BUILD.bazel").is_file());
        assert!(!temp.path().join(".rebaze").exists());
    }

    #[test]
    fn workspace_only_validation_does_not_promote_missing_build_evidence() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(
            temp.path().join("MODULE.bazel"),
            "module(name = \"example\")",
        )
        .unwrap();
        let report = validate_with_report(&crate::ValidateOptions {
            path: temp.path().to_str().unwrap(),
            unsafe_mode: true,
        })
        .unwrap();
        assert_eq!(report.claims.aggregate(), ClaimResult::Unknown);
        assert_eq!(report.outputs, []);
        assert!(report.stages.contains(&StageRecord {
            stage: Stage::GeneratedBuild,
            outcome: StageOutcome::Skipped(SkipReason::ValidationDisabled)
        }));
    }

    #[cfg(unix)]
    #[test]
    fn application_error_retains_intent_without_promoting_build_claims() {
        let temp = input();
        let outside = temp.path().join("outside");
        std::fs::write(&outside, "preserve").unwrap();
        std::os::unix::fs::symlink(&outside, temp.path().join("BUILD.bazel")).unwrap();
        let failure = migrate_with_report(&options(temp.path(), false, true)).unwrap_err();
        assert_eq!(failure.report.outputs.len(), 4);
        assert_eq!(failure.report.claims.aggregate(), ClaimResult::Unknown);
        assert!(matches!(
            failure.report.stages.last(),
            Some(StageRecord {
                stage: Stage::Application,
                outcome: StageOutcome::Failed { .. }
            })
        ));
        assert_eq!(std::fs::read_to_string(outside).unwrap(), "preserve");
        assert!(!temp.path().join(".bazelrc").exists());
    }
}
