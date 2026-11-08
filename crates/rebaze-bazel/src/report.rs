//! Pure lowering results, distinct from application and build verification.

use std::path::PathBuf;

use crate::config::{ConfigError, OmissionReason};
use crate::{GenerationError, NativeLinkError, OutputBundle};

/// Stable diagnostic categories; presentation text may evolve independently.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticCode {
    Configuration,
    SourceEvaluation,
    DependencyPlan,
    NativeLink,
    TestSemantics,
    Discovery,
    Rendering,
    OutputBundle,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiagnosticOrigin {
    Source {
        path: PathBuf,
        character_offset: usize,
    },
    File(PathBuf),
    Target(String),
    ConfigurationField(String),
    Output(String),
    /// No more precise origin is available in the current source model.
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub code: DiagnosticCode,
    pub origin: DiagnosticOrigin,
    pub message: String,
}

/// An observed mapping decision, without claiming the destination was built.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DependencyDecision {
    pub target: String,
    pub input: String,
    pub resolution: DependencyResolution,
}

/// Successful lowering can map or deliberately omit a link; unresolved is an error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DependencyResolution {
    Mapped(String),
    Omitted(OmissionReason),
}

/// Native CMake direct input-link decisions only. JVM and provider transitives
/// currently supply no decisions here; emptiness is not a completeness claim.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LoweringReport {
    pub dependencies: Vec<DependencyDecision>,
}

/// Complete generated intent. This type records no successful application/checks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenerationOutput {
    pub bundle: OutputBundle,
    pub report: LoweringReport,
}

impl GenerationError {
    pub fn diagnostic(&self) -> Diagnostic {
        let (code, origin) = match self {
            Self::Config(error) => (
                DiagnosticCode::Configuration,
                match error {
                    ConfigError::Read { path, .. } | ConfigError::Parse { path, .. } => {
                        DiagnosticOrigin::File(path.clone())
                    }
                    ConfigError::UnsupportedOverride(field)
                    | ConfigError::InvalidValue { field, .. } => {
                        DiagnosticOrigin::ConfigurationField((*field).into())
                    }
                    ConfigError::UnsupportedProfile(_) => DiagnosticOrigin::Unknown,
                },
            ),
            Self::Source(issue) => (
                DiagnosticCode::SourceEvaluation,
                DiagnosticOrigin::Source {
                    path: issue.path.clone(),
                    character_offset: issue.offset,
                },
            ),
            Self::NativeLink(error) => (
                DiagnosticCode::NativeLink,
                match error {
                    NativeLinkError::Unresolved { target, .. }
                    | NativeLinkError::AliasUnresolved { target, .. }
                    | NativeLinkError::CommentMapping { target, .. }
                    | NativeLinkError::ExecutableLink { target, .. }
                    | NativeLinkError::MissingTarget(target) => {
                        DiagnosticOrigin::Target(target.clone())
                    }
                    _ => DiagnosticOrigin::Unknown,
                },
            ),
            Self::Plan(_) => (DiagnosticCode::DependencyPlan, DiagnosticOrigin::Unknown),
            Self::Test(_) => (DiagnosticCode::TestSemantics, DiagnosticOrigin::Unknown),
            Self::Discovery(_) => (DiagnosticCode::Discovery, DiagnosticOrigin::Unknown),
            Self::Render(_) => (DiagnosticCode::Rendering, DiagnosticOrigin::Unknown),
            Self::Gradle(_) => (DiagnosticCode::SourceEvaluation, DiagnosticOrigin::Unknown),
            Self::Bundle(error) => (
                DiagnosticCode::OutputBundle,
                match error {
                    crate::BundleError::InvalidPath(path) | crate::BundleError::Duplicate(path) => {
                        DiagnosticOrigin::Output(path.clone())
                    }
                    crate::BundleError::FileAncestor { descendant, .. } => {
                        DiagnosticOrigin::Output(descendant.clone())
                    }
                    crate::BundleError::CaseAlias { second, .. } => {
                        DiagnosticOrigin::Output(second.clone())
                    }
                    crate::BundleError::Empty => DiagnosticOrigin::Unknown,
                },
            ),
        };
        Diagnostic {
            code,
            origin,
            message: self.to_string(),
        }
    }
}
