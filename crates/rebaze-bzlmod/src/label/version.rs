//! Version parsing and comparison.
// TODO: Implement full Bazel version comparison - delegate to subagent

use serde::{Deserialize, Serialize};
use std::fmt;

/// A Bazel module version.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Version(String);

impl Version {
    /// Create a new version.
    pub fn new(version: impl Into<String>) -> Result<Self, crate::Error> {
        let version = version.into();
        if version.is_empty() {
            return Err(crate::Error::InvalidVersion("empty version".into()));
        }
        Ok(Self(version))
    }

    /// Get the version as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl AsRef<str> for Version {
    fn as_ref(&self) -> &str {
        &self.0
    }
}
