//! Actual operation stages and verification evidence, distinct from output intent.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use rebaze_bazel::{Diagnostic, DiscoverySnapshot, LoweringReport, OutputBundle, OutputPath};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum BuildClaim {
    OriginalBuild,
    GeneratedBuild,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaimResult {
    Passed,
    Failed,
    Unknown,
}

/// Missing evidence, including an empty required set, can never aggregate to pass.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClaimResults {
    required: BTreeSet<BuildClaim>,
    results: BTreeMap<BuildClaim, ClaimResult>,
}

impl ClaimResults {
    pub fn new(required: impl IntoIterator<Item = BuildClaim>) -> Self {
        Self {
            required: required.into_iter().collect(),
            results: BTreeMap::new(),
        }
    }

    pub fn require(&mut self, claim: BuildClaim) {
        self.required.insert(claim);
    }

    pub fn record(&mut self, claim: BuildClaim, result: ClaimResult) {
        self.results.insert(claim, result);
    }

    pub fn required(&self) -> impl ExactSizeIterator<Item = &BuildClaim> {
        self.required.iter()
    }

    pub fn result(&self, claim: BuildClaim) -> ClaimResult {
        self.results
            .get(&claim)
            .copied()
            .unwrap_or(ClaimResult::Unknown)
    }

    pub fn aggregate(&self) -> ClaimResult {
        if self.required.is_empty() {
            return ClaimResult::Unknown;
        }
        let mut missing = false;
        for claim in &self.required {
            match self.result(*claim) {
                ClaimResult::Failed => return ClaimResult::Failed,
                ClaimResult::Unknown => missing = true,
                ClaimResult::Passed => {}
            }
        }
        if missing {
            ClaimResult::Unknown
        } else {
            ClaimResult::Passed
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    BuildSystemSelection,
    Settings,
    GeneratorSelection,
    Capture,
    Preparation,
    HostDiscovery,
    OriginalBuild,
    Rendering,
    Application,
    GeneratedBuild,
    ExternalGeneration,
    WorkspaceInspection,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkipReason {
    Preview,
    ValidationDisabled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StageOutcome {
    Passed,
    Failed {
        message: String,
        diagnostic: Option<Diagnostic>,
    },
    Skipped(SkipReason),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StageRecord {
    pub stage: Stage,
    pub outcome: StageOutcome,
}

/// Planned argument vectors, never a shell command or a successful process claim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessPreview {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExistingFilePolicy {
    Preserve,
    RequireBazel9,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScaffoldingPreview {
    pub path: OutputPath,
    pub content: String,
    pub existing: ExistingFilePolicy,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MigrationPreview {
    Files(OutputBundle),
    /// External generation has not supplied a complete validated output bundle.
    External {
        commands: Vec<ProcessPreview>,
        scaffolding: Vec<ScaffoldingPreview>,
    },
}

/// Discovery policy and actual observations are explicit, independently of lowering.
#[derive(Debug, Clone)]
pub struct DiscoveryReport {
    pub requests: Vec<rebaze_cmake::PkgConfigModule>,
    pub command_timeout: std::time::Duration,
    pub max_output_bytes: usize,
    pub observations: Option<DiscoverySnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceEvidence {
    CMakeSource,
    CMakeFileApi {
        build_dir: PathBuf,
        requested_configuration: Option<String>,
        selected_configuration: rebaze_cmake_file_api::SelectedConfiguration,
    },
    GradleSource,
}

#[derive(Debug, Clone)]
pub struct MigrationReport {
    pub root: Option<PathBuf>,
    pub source: Option<SourceEvidence>,
    pub generator: Option<crate::BuildGenerator>,
    pub stages: Vec<StageRecord>,
    pub claims: ClaimResults,
    pub lowering: Option<LoweringReport>,
    pub discovery: Option<DiscoveryReport>,
    /// Intended paths only; an application failure can leave a partial batch.
    pub outputs: Vec<OutputPath>,
    pub preview: Option<MigrationPreview>,
}

impl Default for MigrationReport {
    fn default() -> Self {
        Self {
            root: None,
            source: None,
            generator: None,
            stages: Vec::new(),
            claims: ClaimResults::new([BuildClaim::GeneratedBuild]),
            lowering: None,
            discovery: None,
            outputs: Vec::new(),
            preview: None,
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("operation failed: {source}")]
pub struct MigrationFailure {
    pub report: Box<MigrationReport>,
    #[source]
    pub source: anyhow::Error,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absence_and_empty_requirements_are_unknown() {
        assert_eq!(ClaimResults::default().aggregate(), ClaimResult::Unknown);
        let mut claims = ClaimResults::new([BuildClaim::OriginalBuild, BuildClaim::GeneratedBuild]);
        assert_eq!(claims.aggregate(), ClaimResult::Unknown);
        claims.record(BuildClaim::GeneratedBuild, ClaimResult::Passed);
        assert_eq!(claims.aggregate(), ClaimResult::Unknown);
        assert_eq!(
            claims.result(BuildClaim::OriginalBuild),
            ClaimResult::Unknown
        );
        claims.record(BuildClaim::OriginalBuild, ClaimResult::Passed);
        assert_eq!(claims.aggregate(), ClaimResult::Passed);
        claims.record(BuildClaim::GeneratedBuild, ClaimResult::Failed);
        assert_eq!(claims.aggregate(), ClaimResult::Failed);
    }

    #[test]
    fn unrequired_success_cannot_fill_a_missing_claim() {
        let mut claims = ClaimResults::new([BuildClaim::OriginalBuild]);
        claims.record(BuildClaim::GeneratedBuild, ClaimResult::Passed);
        assert_eq!(claims.aggregate(), ClaimResult::Unknown);
    }
}
