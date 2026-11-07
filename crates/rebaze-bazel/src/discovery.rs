//! Explicit host observations consumed by native generation.
//!
//! This module owns data and validation only. Probing an installed package tool
//! belongs to the caller's I/O boundary, such as `rebaze_core::discovery`.

use std::collections::BTreeMap;
use std::fmt;

use rebaze_cmake::PkgConfigModule;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Observed flags and optional filesystem layout for a pkg-config request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageInfo {
    pub cflags: Vec<String>,
    pub linkopts: Vec<String>,
    /// No prefix is a valid observation; the system wrapper then needs configuration.
    pub system: Option<SystemLibrary>,
}

/// A system library rooted at an observed installation prefix.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SystemLibrary {
    pub prefix: String,
    /// Paths under the prefix are relative; paths outside it stay absolute.
    pub include_dirs: Vec<String>,
    pub lib_dirs: Vec<String>,
    pub libs: Vec<String>,
}

/// A package lookup that could not establish availability.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum UnavailableReason {
    ToolNotFound,
    UnsatisfiedRequest { diagnostic: String },
}

impl fmt::Display for UnavailableReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ToolNotFound => f.write_str("pkg-config executable not found"),
            Self::UnsatisfiedRequest { diagnostic } => {
                write!(f, "packages unavailable: {diagnostic:?}")
            }
        }
    }
}

/// Operation performed by the discovery adapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProbeOperation {
    Exists,
    Cflags,
    Libs,
    Prefix,
}

impl fmt::Display for ProbeOperation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}

/// Bounded output stream of a probe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProbeStream {
    Stdout,
    Stderr,
}

impl fmt::Display for ProbeStream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Stdout => f.write_str("stdout"),
            Self::Stderr => f.write_str("stderr"),
        }
    }
}

/// A probe failure distinct from a tool/package that is unavailable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Error)]
pub enum ProbeFailure {
    #[error("invalid pkg-config request: {0}")]
    InvalidRequest(String),
    #[error("could not execute probe: {0}")]
    Io(String),
    #[error("probe exceeded its command deadline")]
    Timeout,
    #[error("probe {stream} exceeded {limit} bytes")]
    OutputLimit { stream: ProbeStream, limit: usize },
    #[error("probe {0} is not UTF-8")]
    InvalidUtf8(ProbeStream),
    #[error("malformed shell-escaped flags in probe output")]
    InvalidFlags,
    #[error("probe prefix is not a single absolute path")]
    InvalidPrefix,
    #[error("{operation} probe failed (exit {code:?}): {diagnostic:?}")]
    CommandFailed {
        operation: ProbeOperation,
        code: Option<i32>,
        diagnostic: String,
    },
}

/// Discovery is an observation, not a dependency-resolution strategy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProbeOutcome {
    Available(PackageInfo),
    Unavailable(UnavailableReason),
    Failed(ProbeFailure),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Entry {
    packages: Vec<String>,
    outcome: ProbeOutcome,
}

/// Owned, serializable observations keyed by normalized target name.
///
/// An omitted request is an error. Record unavailability explicitly to opt into
/// the existing fallback/stub behavior. Entries retain their exact request so a
/// stale or mismatched snapshot cannot silently supply unrelated host flags.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscoverySnapshot {
    entries: BTreeMap<String, Entry>,
}

/// Incomplete, conflicting, or failed discovery supplied to generation.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum SnapshotError {
    #[error("missing discovery for {module:?}; supply an explicit probe outcome")]
    Missing { module: String },
    #[error("discovery request does not match packages for {module:?}")]
    RequestMismatch { module: String },
    #[error("package discovery failed for {module:?}: {failure}")]
    Failed {
        module: String,
        failure: ProbeFailure,
    },
}

/// Normalize the target prefix exactly as the native generator does.
#[must_use]
pub fn target_name(prefix: &str) -> String {
    prefix.to_lowercase().replace('-', "_")
}

impl DiscoverySnapshot {
    /// Record or replace an observation for the same exact package request.
    pub fn record(
        &mut self,
        module: &PkgConfigModule,
        outcome: ProbeOutcome,
    ) -> Result<(), SnapshotError> {
        let name = target_name(&module.prefix);
        if self
            .entries
            .get(&name)
            .is_some_and(|entry| entry.packages != module.packages)
        {
            return Err(SnapshotError::RequestMismatch { module: name });
        }
        self.entries.insert(
            name,
            Entry {
                packages: module.packages.clone(),
                outcome,
            },
        );
        Ok(())
    }

    /// Read the exact requested observation, without inspecting the environment.
    pub fn outcome(&self, module: &PkgConfigModule) -> Result<&ProbeOutcome, SnapshotError> {
        let name = target_name(&module.prefix);
        let entry = self
            .entries
            .get(&name)
            .ok_or_else(|| SnapshotError::Missing {
                module: name.clone(),
            })?;
        if entry.packages != module.packages {
            return Err(SnapshotError::RequestMismatch { module: name });
        }
        Ok(&entry.outcome)
    }

    /// Return available data, explicit unavailability, or an actionable failure.
    pub fn package(&self, module: &PkgConfigModule) -> Result<Option<&PackageInfo>, SnapshotError> {
        match self.outcome(module)? {
            ProbeOutcome::Available(info) => Ok(Some(info)),
            ProbeOutcome::Unavailable(_) => Ok(None),
            ProbeOutcome::Failed(failure) => Err(SnapshotError::Failed {
                module: target_name(&module.prefix),
                failure: failure.clone(),
            }),
        }
    }
}
