use anyhow::Result;
use clap::{Parser, Subcommand, ValueEnum};
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(name = "rebaze")]
#[command(about = "Migrate from Gradle, CMake, and other build tools to Bazel")]
#[command(version)]
struct Cli {
    /// Enable verbose output
    #[arg(short, long, global = true)]
    verbose: bool,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Copy, Clone, Debug, ValueEnum)]
enum BuildGeneratorArg {
    Auto,
    Native,
    Bazelle,
}

impl From<BuildGeneratorArg> for rebaze_core::BuildGenerator {
    fn from(value: BuildGeneratorArg) -> Self {
        match value {
            BuildGeneratorArg::Auto => Self::Auto,
            BuildGeneratorArg::Native => Self::Native,
            BuildGeneratorArg::Bazelle => Self::Bazelle,
        }
    }
}

#[derive(Subcommand)]
enum Commands {
    /// Analyze a project and detect its build system
    Analyze {
        /// Path to the project root
        #[arg(default_value = ".")]
        path: String,
    },

    /// Migrate a project to Bazel
    Migrate {
        /// Path to the project root
        #[arg(default_value = ".")]
        path: String,

        /// Source build system (auto-detected if not specified)
        #[arg(short, long)]
        from: Option<String>,

        /// Path to configuration file (default: rebaze.toml in project root)
        ///
        /// The config file controls dependency versions, library mappings,
        /// filter rules, and other migration options.
        #[arg(short, long)]
        config: Option<String>,

        /// Dry run - show what would be generated without writing files
        #[arg(long)]
        dry_run: bool,

        /// CMake build directory containing cmake-file-api replies
        #[arg(long)]
        cmake_build_dir: Option<String>,

        /// CMake configuration name to select from the File API codemodel (e.g. Debug)
        #[arg(long)]
        cmake_config: Option<String>,

        /// Fail if cmake-file-api is unavailable instead of falling back to parsing
        #[arg(long)]
        cmake_file_api_only: bool,

        /// Build file generator (auto prefers bazelle when available)
        #[arg(long, value_enum, default_value = "auto")]
        build_generator: BuildGeneratorArg,

        /// Path to a bazelle workspace to build the bazelle binary
        #[arg(long)]
        bazelle_root: Option<String>,

        /// Path to a bazelle binary
        #[arg(long)]
        bazelle_bin: Option<String>,

        /// Skip pre/post build validation checks
        #[arg(long)]
        unsafe_mode: bool,
    },

    /// Validate generated Bazel files
    Validate {
        /// Path to the project root
        #[arg(default_value = ".")]
        path: String,

        /// Skip Bazel build validation (only checks workspace presence)
        #[arg(long)]
        unsafe_mode: bool,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    // Initialize tracing
    let filter = if cli.verbose {
        EnvFilter::new("debug")
    } else {
        EnvFilter::new("info")
    };

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .with_writer(std::io::stderr)
        .init();

    match cli.command {
        Commands::Analyze { path } => {
            tracing::info!("Analyzing project at: {}", path);
            let result = rebaze_core::analyze(&path)?;
            println!("{result}");
        }
        Commands::Migrate {
            path,
            from,
            config,
            dry_run,
            cmake_build_dir,
            cmake_config,
            cmake_file_api_only,
            build_generator,
            bazelle_root,
            bazelle_bin,
            unsafe_mode,
        } => {
            tracing::info!("Migrating project at: {}", path);
            if dry_run {
                tracing::info!("Dry run mode - no files will be written");
            }

            // Load configuration
            let project_path = std::path::Path::new(&path);
            let migration_config = rebaze_core::Config::load_from_path_or_discover(
                config.as_ref().map(std::path::Path::new),
                project_path,
            )?;

            let options = rebaze_core::MigrateOptions {
                path: &path,
                from: from.as_deref(),
                dry_run,
                cmake_build_dir: cmake_build_dir.as_deref(),
                cmake_config: cmake_config.as_deref(),
                cmake_file_api_only,
                build_generator: build_generator.into(),
                bazelle_root: bazelle_root.as_deref(),
                bazelle_bin: bazelle_bin.as_deref(),
                unsafe_mode,
                config: Some(migration_config),
            };
            let report = rebaze_core::migrate_with_report(&options)?;
            present_preview(&report, &mut std::io::stdout().lock())?;
            if !dry_run {
                present_checks(&report)?;
            }
        }
        Commands::Validate { path, unsafe_mode } => {
            tracing::info!("Validating Bazel files at: {}", path);
            let options = rebaze_core::ValidateOptions {
                path: &path,
                unsafe_mode,
            };
            let report = rebaze_core::validate_with_report(&options)?;
            present_checks(&report)?;
        }
    }

    Ok(())
}

// Process previews escape whitespace/control characters and show argument vectors,
// rather than presenting project data as a copyable shell program.
#[allow(clippy::unnecessary_debug_formatting)]
fn present_preview(
    report: &rebaze_core::report::MigrationReport,
    output: &mut impl std::io::Write,
) -> std::io::Result<()> {
    use rebaze_core::report::{ExistingFilePolicy, MigrationPreview};
    if let Some(preview) = &report.preview {
        match preview {
            MigrationPreview::Files(bundle) => {
                for (path, content) in bundle.iter() {
                    writeln!(output, "--- {} ---", path.as_str())?;
                    writeln!(output, "{content}")?;
                }
            }
            MigrationPreview::External {
                commands,
                scaffolding,
            } => {
                for command in commands {
                    writeln!(
                        output,
                        "Planned process in {:?}: {:?} {:?}",
                        command.cwd, command.program, command.args
                    )?;
                }
                for file in scaffolding {
                    let policy = match file.existing {
                        ExistingFilePolicy::Preserve => "preserve an existing file",
                        ExistingFilePolicy::RequireBazel9 => "preserve an existing Bazel 9 version",
                    };
                    writeln!(
                        output,
                        "--- {} (create if absent; {policy}) ---",
                        file.path.as_str()
                    )?;
                    writeln!(output, "{}", file.content)?;
                }
            }
        }
    }
    Ok(())
}

fn present_checks(report: &rebaze_core::report::MigrationReport) -> Result<()> {
    use rebaze_core::report::{BuildClaim, ClaimResult, SkipReason, Stage, StageOutcome};
    match report.claims.aggregate() {
        ClaimResult::Passed => tracing::info!("Selected build checks passed"),
        ClaimResult::Unknown => {
            anyhow::ensure!(
                report.claims.required().len() > 0,
                "Required build-check set is empty"
            );
            for claim in report.claims.required() {
                if report.claims.result(*claim) == ClaimResult::Unknown {
                    let stage = match claim {
                        BuildClaim::OriginalBuild => Stage::OriginalBuild,
                        BuildClaim::GeneratedBuild => Stage::GeneratedBuild,
                    };
                    anyhow::ensure!(
                        report.stages.iter().any(|record| record.stage == stage
                            && matches!(
                                record.outcome,
                                StageOutcome::Skipped(SkipReason::ValidationDisabled)
                            )),
                        "Required build-check evidence is missing for {claim:?}"
                    );
                }
            }
            tracing::info!("Build validation was skipped (--unsafe-mode)");
        }
        ClaimResult::Failed => anyhow::bail!("Selected build checks failed"),
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use rebaze_core::report::{
        BuildClaim, ClaimResult, ClaimResults, MigrationPreview, MigrationReport, SkipReason,
        Stage, StageOutcome, StageRecord,
    };

    struct FailedWriter;
    impl std::io::Write for FailedWriter {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("fixture"))
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn missing_evidence_is_an_error_unless_validation_was_explicitly_skipped() {
        let mut report = MigrationReport::default();
        assert!(present_checks(&report).is_err());
        report.claims = ClaimResults::default();
        assert!(present_checks(&report).is_err());
        report.stages.push(StageRecord {
            stage: Stage::GeneratedBuild,
            outcome: StageOutcome::Skipped(SkipReason::ValidationDisabled),
        });
        assert!(present_checks(&report).is_err());
        assert_eq!(report.claims.aggregate(), ClaimResult::Unknown);
        report.claims = ClaimResults::new([BuildClaim::GeneratedBuild]);
        assert!(present_checks(&report).is_ok());
        report.claims.require(BuildClaim::OriginalBuild);
        assert!(present_checks(&report).is_err());
        report
            .claims
            .record(BuildClaim::GeneratedBuild, ClaimResult::Failed);
        assert!(present_checks(&report).is_err());
    }

    #[test]
    fn preview_returns_bytes_through_the_selected_writer() {
        let report = MigrationReport {
            preview: Some(MigrationPreview::Files(
                rebaze_bazel::OutputBundle::new([("BUILD.bazel".into(), "# data\n".into())])
                    .unwrap(),
            )),
            ..MigrationReport::default()
        };
        let mut bytes = Vec::new();
        present_preview(&report, &mut bytes).unwrap();
        assert_eq!(
            String::from_utf8(bytes).unwrap(),
            "--- BUILD.bazel ---\n# data\n\n"
        );
        assert!(present_preview(&report, &mut FailedWriter).is_err());
    }
}
