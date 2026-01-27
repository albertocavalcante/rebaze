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
            rebaze_core::migrate(&options)?;
        }
        Commands::Validate { path, unsafe_mode } => {
            tracing::info!("Validating Bazel files at: {}", path);
            let options = rebaze_core::ValidateOptions {
                path: &path,
                unsafe_mode,
            };
            rebaze_core::validate(&options)?;
        }
    }

    Ok(())
}
