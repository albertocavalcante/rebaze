//! Explicit, bounded pkg-config discovery for native migration.
//!
//! The executable and its environment are trusted host inputs. These limits
//! bound capture and the direct child; this adapter is not a process sandbox.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::time::Duration;

use rebaze_bazel::discovery::{
    DiscoverySnapshot, PackageInfo, ProbeFailure, ProbeOperation, ProbeOutcome, ProbeStream,
    SnapshotError, SystemLibrary, UnavailableReason,
};
use rebaze_cmake::PkgConfigModule;
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;

/// Limits and explicit executable selection for one discovery pass.
#[derive(Debug, Clone)]
pub struct DiscoveryOptions {
    pub program: PathBuf,
    /// Deadline includes process exit and draining both output streams.
    pub command_timeout: Duration,
    /// Maximum retained bytes per output stream, before UTF-8/flag parsing.
    pub max_output_bytes: usize,
}

impl Default for DiscoveryOptions {
    fn default() -> Self {
        Self {
            program: "pkg-config".into(),
            command_timeout: Duration::from_secs(5),
            max_output_bytes: 256 * 1024,
        }
    }
}

/// Failure to start a discovery pass. Per-package failures live in the snapshot.
#[derive(Debug, Error)]
pub enum DiscoveryError {
    #[error("probe timeout must be in (0, 300s] and output limit in [1, 16 MiB]")]
    InvalidLimits,
    #[error("could not initialize package discovery: {0}")]
    Initialize(#[source] std::io::Error),
    #[error("package discovery worker panicked")]
    WorkerPanicked,
    #[error(transparent)]
    Snapshot(#[from] SnapshotError),
}

/// Observe each distinct package request once at the explicit host boundary.
///
/// Missing tools/packages are explicit fallback observations. Failed probes stay
/// errors when generation consumes the snapshot. No project outputs are created.
///
/// A dedicated runtime thread keeps this synchronous API usable by callers that
/// already run inside an async runtime, without nested-runtime panics.
pub fn discover_packages(
    modules: &[PkgConfigModule],
    options: &DiscoveryOptions,
) -> Result<DiscoverySnapshot, DiscoveryError> {
    if options.command_timeout.is_zero()
        || options.command_timeout > Duration::from_secs(300)
        || !(1..=16 * 1024 * 1024).contains(&options.max_output_bytes)
    {
        return Err(DiscoveryError::InvalidLimits);
    }
    let modules = rebaze_bazel::deduplicate_modules(modules)?;
    if modules.is_empty() {
        return Ok(DiscoverySnapshot::default());
    }
    std::thread::scope(|scope| {
        std::thread::Builder::new()
            .name("rebaze-discovery".into())
            .spawn_scoped(scope, || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(DiscoveryError::Initialize)?;
                runtime.block_on(discover_with(&modules, &mut HostRunner { options }))
            })
            .map_err(DiscoveryError::Initialize)?
            .join()
            .map_err(|_| DiscoveryError::WorkerPanicked)?
    })
}

#[derive(Debug)]
struct ProbeOutput {
    status: ExitStatus,
    stdout: String,
    stderr: String,
}

// A private seam for deterministic adapter tests; no public plugin framework is needed.
trait ProbeRunner {
    async fn run(
        &mut self,
        operation: ProbeOperation,
        packages: &[String],
    ) -> Result<ProbeOutput, RunError>;
}

#[derive(Debug)]
enum RunError {
    ToolNotFound,
    Failed(ProbeFailure),
}

struct HostRunner<'a> {
    options: &'a DiscoveryOptions,
}

impl ProbeRunner for HostRunner<'_> {
    async fn run(
        &mut self,
        operation: ProbeOperation,
        packages: &[String],
    ) -> Result<ProbeOutput, RunError> {
        let mut command = Command::new(&self.options.program);
        match operation {
            ProbeOperation::Exists => command.args(["--exists", "--print-errors"]),
            ProbeOperation::Cflags => command.arg("--cflags"),
            ProbeOperation::Libs => command.arg("--libs"),
            ProbeOperation::Prefix => command.arg("--variable=prefix"),
        };
        command.args(packages);
        run_command(command, self.options).await
    }
}

async fn read_bounded(
    stream: impl AsyncRead + Unpin,
    name: ProbeStream,
    limit: usize,
) -> Result<String, ProbeFailure> {
    let mut bytes = Vec::new();
    // Validated options bound this conversion and allocation to 16 MiB + 1.
    stream
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .await
        .map_err(|error| ProbeFailure::Io(error.to_string()))?;
    if bytes.len() > limit {
        return Err(ProbeFailure::OutputLimit {
            stream: name,
            limit,
        });
    }
    String::from_utf8(bytes).map_err(|_| ProbeFailure::InvalidUtf8(name))
}

async fn run_command(
    mut command: Command,
    options: &DiscoveryOptions,
) -> Result<ProbeOutput, RunError> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                RunError::ToolNotFound
            } else {
                RunError::Failed(ProbeFailure::Io(error.to_string()))
            }
        })?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| RunError::Failed(ProbeFailure::Io("missing stdout pipe".into())))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| RunError::Failed(ProbeFailure::Io("missing stderr pipe".into())))?;
    let work = async {
        let (status, stdout, stderr) = tokio::try_join!(
            async {
                child
                    .wait()
                    .await
                    .map_err(|error| ProbeFailure::Io(error.to_string()))
            },
            read_bounded(stdout, ProbeStream::Stdout, options.max_output_bytes),
            read_bounded(stderr, ProbeStream::Stderr, options.max_output_bytes),
        )?;
        Ok(ProbeOutput {
            status,
            stdout,
            stderr,
        })
    };
    let result = match tokio::time::timeout(options.command_timeout, work).await {
        Ok(result) => result,
        Err(_) => Err(ProbeFailure::Timeout),
    };
    if result.is_err() {
        // kill().await terminates AND reaps the direct child. Dropped pipe futures
        // cannot leave blocking reader threads behind if inherited handles stay open.
        match child.try_wait() {
            Ok(Some(_)) => {}
            Ok(None) => {
                if let Err(error) = child.kill().await {
                    // The child may exit between try_wait and kill, particularly
                    // on Windows. Preserve the original failure when it is reaped.
                    if !matches!(child.try_wait(), Ok(Some(_))) {
                        return Err(RunError::Failed(ProbeFailure::Io(format!(
                            "could not stop probe: {error}"
                        ))));
                    }
                }
            }
            Err(error) => return Err(RunError::Failed(ProbeFailure::Io(error.to_string()))),
        }
    }
    result.map_err(RunError::Failed)
}

async fn discover_with(
    modules: &[PkgConfigModule],
    runner: &mut impl ProbeRunner,
) -> Result<DiscoverySnapshot, DiscoveryError> {
    let mut snapshot = DiscoverySnapshot::default();
    let mut observations = BTreeMap::new();
    for module in modules {
        let outcome = if let Some(outcome) = observations.get(&module.packages) {
            Clone::clone(outcome)
        } else {
            let outcome = match probe(&module.packages, runner).await {
                Ok(info) => ProbeOutcome::Available(info),
                Err(ProbeError::Unavailable(reason)) => ProbeOutcome::Unavailable(reason),
                Err(ProbeError::Failed(failure)) => ProbeOutcome::Failed(failure),
            };
            observations.insert(module.packages.clone(), outcome.clone());
            outcome
        };
        snapshot.record(module, outcome)?;
    }
    Ok(snapshot)
}

#[derive(Debug)]
enum ProbeError {
    Unavailable(UnavailableReason),
    Failed(ProbeFailure),
}

impl From<RunError> for ProbeError {
    fn from(error: RunError) -> Self {
        match error {
            RunError::ToolNotFound => Self::Unavailable(UnavailableReason::ToolNotFound),
            RunError::Failed(error) => Self::Failed(error),
        }
    }
}

fn validate_request(packages: &[String]) -> Result<(), ProbeError> {
    // This is a command-argument budget, not a limit on project/package graph size.
    if packages.is_empty()
        || packages
            .iter()
            .fold(0usize, |bytes, package| bytes.saturating_add(package.len()))
            > 64 * 1024
        || packages.iter().any(|package| {
            let package = package.trim();
            package.is_empty() || package.starts_with('-') || package.chars().any(char::is_control)
        })
    {
        return Err(ProbeError::Failed(ProbeFailure::InvalidRequest(
            "expected nonempty package expressions, no options/control characters, at most 64 KiB"
                .into(),
        )));
    }
    Ok(())
}

async fn query(
    runner: &mut impl ProbeRunner,
    operation: ProbeOperation,
    packages: &[String],
) -> Result<String, ProbeError> {
    let output = runner
        .run(operation, packages)
        .await
        .map_err(|error| match error {
            RunError::ToolNotFound => ProbeError::Failed(ProbeFailure::Io(format!(
                "executable became unavailable during {operation} probe"
            ))),
            RunError::Failed(failure) => ProbeError::Failed(failure),
        })?;
    if !output.status.success() {
        return Err(ProbeError::Failed(ProbeFailure::CommandFailed {
            operation,
            code: output.status.code(),
            diagnostic: output.stderr,
        }));
    }
    Ok(output.stdout)
}

async fn probe(
    packages: &[String],
    runner: &mut impl ProbeRunner,
) -> Result<PackageInfo, ProbeError> {
    validate_request(packages)?;
    let exists = runner.run(ProbeOperation::Exists, packages).await?;
    if !exists.status.success() {
        // pkg-config uses exit 1 for unsatisfied package/version requirements.
        if exists.status.code() == Some(1) {
            return Err(ProbeError::Unavailable(
                UnavailableReason::UnsatisfiedRequest {
                    diagnostic: exists.stderr,
                },
            ));
        }
        return Err(ProbeError::Failed(ProbeFailure::CommandFailed {
            operation: ProbeOperation::Exists,
            code: exists.status.code(),
            diagnostic: exists.stderr,
        }));
    }
    let cflags = parse_flags(&query(runner, ProbeOperation::Cflags, packages).await?)?;
    let linkopts = parse_flags(&query(runner, ProbeOperation::Libs, packages).await?)?;
    // Prefix is queried only for the first package, avoiding a multi-line prefix
    // when one pkg_check_modules request includes several package expressions.
    let prefix = query(runner, ProbeOperation::Prefix, &packages[..1]).await?;
    let prefix = prefix.trim();
    let system = if prefix.is_empty() {
        None
    } else {
        if !Path::new(prefix).is_absolute() || prefix.chars().any(char::is_control) {
            return Err(ProbeError::Failed(ProbeFailure::InvalidPrefix));
        }
        Some(SystemLibrary {
            prefix: prefix.into(),
            include_dirs: flag_values(&cflags, "-I")
                .map(|path| relative_path(path, prefix))
                .collect(),
            lib_dirs: flag_values(&linkopts, "-L")
                .map(|path| relative_path(path, prefix))
                .collect(),
            libs: flag_values(&linkopts, "-l").map(str::to_owned).collect(),
        })
    };
    Ok(PackageInfo {
        cflags,
        linkopts,
        system,
    })
}

fn parse_flags(source: &str) -> Result<Vec<String>, ProbeError> {
    let flags = shlex::split(source).ok_or(ProbeError::Failed(ProbeFailure::InvalidFlags))?;
    let mut flags = flags.into_iter();
    let mut normalized = Vec::new();
    while let Some(flag) = flags.next() {
        if matches!(flag.as_str(), "-I" | "-L" | "-l") {
            let value = flags
                .next()
                .filter(|value| !value.is_empty() && !value.starts_with('-'))
                .ok_or(ProbeError::Failed(ProbeFailure::InvalidFlags))?;
            normalized.push(format!("{flag}{value}"));
        } else {
            normalized.push(flag);
        }
    }
    Ok(normalized)
}

fn flag_values<'a>(flags: &'a [String], prefix: &'a str) -> impl Iterator<Item = &'a str> {
    flags
        .iter()
        .filter_map(move |flag| flag.strip_prefix(prefix))
        .filter(|value| !value.is_empty())
}

fn relative_path(path: &str, prefix: &str) -> String {
    Path::new(path).strip_prefix(prefix).map_or_else(
        |_| path.to_owned(),
        |relative| relative.to_string_lossy().into_owned(),
    )
}

#[cfg(test)]
mod tests;
