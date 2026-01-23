use anyhow::Result;
use clap::{Parser, Subcommand};
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

        /// Dry run - show what would be generated without writing files
        #[arg(long)]
        dry_run: bool,
    },

    /// Validate generated Bazel files
    Validate {
        /// Path to the project root
        #[arg(default_value = ".")]
        path: String,
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
            dry_run,
        } => {
            tracing::info!("Migrating project at: {}", path);
            if dry_run {
                tracing::info!("Dry run mode - no files will be written");
            }
            rebaze_core::migrate(&path, from.as_deref(), dry_run)?;
        }
        Commands::Validate { path } => {
            tracing::info!("Validating Bazel files at: {}", path);
            rebaze_core::validate(&path)?;
        }
    }

    Ok(())
}
