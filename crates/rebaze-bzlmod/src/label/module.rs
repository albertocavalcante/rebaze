//! Module name validation.
// TODO: Implement - delegate to subagent

use serde::{Deserialize, Serialize};
use std::fmt;

/// A validated Bazel module name.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ModuleName(String);

impl ModuleName {
    /// Create a new module name, validating the format.
    pub fn new(name: impl Into<String>) -> Result<Self, crate::Error> {
        let name = name.into();
        // TODO: Full validation per Bazel rules
        if name.is_empty() {
            return Err(crate::Error::InvalidModuleName("empty name".into()));
        }
        Ok(Self(name))
    }

    /// Get the name as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ModuleName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl AsRef<str> for ModuleName {
    fn as_ref(&self) -> &str {
        &self.0
    }
}
