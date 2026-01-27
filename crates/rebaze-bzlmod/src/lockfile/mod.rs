//! MODULE.bazel.lock support.
// TODO: Implement - delegate to subagent

use crate::Result;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

/// Lockfile format version.
pub const LOCKFILE_VERSION: u32 = 26;

/// MODULE.bazel.lock file structure.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Lockfile {
    /// Lockfile format version.
    #[serde(rename = "lockFileVersion")]
    pub version: u32,

    /// Registry file hashes.
    #[serde(rename = "registryFileHashes", default)]
    pub registry_file_hashes: BTreeMap<String, String>,

    /// Selected yanked versions.
    #[serde(rename = "selectedYankedVersions", default)]
    pub selected_yanked_versions: BTreeMap<String, String>,

    /// Module dependencies.
    #[serde(rename = "moduleDepGraph", default)]
    pub module_dep_graph: BTreeMap<String, LockedModule>,

    /// Module extensions.
    #[serde(rename = "moduleExtensions", default)]
    pub module_extensions: BTreeMap<String, serde_json::Value>,
}

/// A locked module entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LockedModule {
    /// Module name.
    pub name: String,
    /// Locked version.
    pub version: String,
    /// Registry URL.
    #[serde(default)]
    pub registry: Option<String>,
    /// Dependencies.
    #[serde(default)]
    pub deps: BTreeMap<String, String>,
}

impl Lockfile {
    /// Create a new empty lockfile.
    pub fn new() -> Self {
        Self {
            version: LOCKFILE_VERSION,
            registry_file_hashes: BTreeMap::new(),
            selected_yanked_versions: BTreeMap::new(),
            module_dep_graph: BTreeMap::new(),
            module_extensions: BTreeMap::new(),
        }
    }

    /// Read lockfile from path.
    pub async fn read(path: &Path) -> Result<Self> {
        let content = tokio::fs::read_to_string(path)
            .await
            .map_err(crate::Error::Io)?;
        let lockfile: Self = serde_json::from_str(&content)?;
        Ok(lockfile)
    }

    /// Write lockfile to path.
    pub async fn write(&self, path: &Path) -> Result<()> {
        let content = serde_json::to_string_pretty(self)?;
        tokio::fs::write(path, content)
            .await
            .map_err(crate::Error::Io)?;
        Ok(())
    }
}

impl Default for Lockfile {
    fn default() -> Self {
        Self::new()
    }
}
