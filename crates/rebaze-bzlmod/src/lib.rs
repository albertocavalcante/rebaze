//! Bazel module dependency resolution using Minimal Version Selection (MVS).
//!
//! This crate is a Rust port of [go-bzlmod](https://github.com/nicholasjng/go-bzlmod),
//! providing Bazel module dependency resolution capabilities.
//!
//! # Features
//!
//! - **Live BCR Resolution**: Fetch module metadata from Bazel Central Registry
//! - **MVS Algorithm**: Implements Russ Cox's Minimal Version Selection
//! - **Multi-Registry Support**: Chain private registries with BCR fallback
//! - **Overrides**: Resolve single-version pins and hydrated Git/local/archive modules
//!   (registry-specific and multiple-version resolution return explicit errors)
//! - **Dependency Graph**: Query paths, explain version selections
//! - **Lockfiles**: Reserved API; read/write support is not implemented
//!
//! # Example
//!
//! ```no_run
//! use rebaze_bzlmod::{Resolver, ResolverOptions};
//!
//! #[tokio::main]
//! async fn main() -> anyhow::Result<()> {
//!     let module_content = r#"
//!         module(name = "my_project", version = "1.0.0")
//!         bazel_dep(name = "rules_rust", version = "0.40.0")
//!     "#;
//!
//!     let result = Resolver::resolve(module_content, ResolverOptions::default()).await?;
//!
//!     for module in &result.modules {
//!         println!("{}@{}", module.name, module.version);
//!     }
//!
//!     Ok(())
//! }
//! ```
//!
//! # Architecture
//!
//! The crate is organized into several modules:
//!
//! - [`label`]: Validated module names and versions
//! - [`parser`]: MODULE.bazel file parsing
//! - [`registry`]: BCR HTTP client
//! - [`resolver`]: MVS resolution algorithm
//! - [`graph`]: Dependency graph and queries
//! - [`lockfile`]: MODULE.bazel.lock support

// Base lint configuration
#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![deny(clippy::all)]
#![warn(clippy::pedantic)]
// Bazel does not inherit the workspace's API/documentation and control-flow lint policy.
#![allow(
    clippy::must_use_candidate,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::doc_markdown,
    clippy::module_name_repetitions,
    clippy::return_self_not_must_use,
    clippy::redundant_else,
    clippy::option_if_let_else,
    clippy::collapsible_if,
    clippy::collapsible_else_if,
    clippy::if_not_else,
    clippy::too_many_lines,
    clippy::manual_let_else,
    clippy::match_same_arms,
    clippy::unused_self,
    clippy::significant_drop_tightening
)]

pub mod error;
pub mod graph;
pub mod label;
pub mod lockfile;
pub mod parser;
pub mod registry;
pub mod resolver;
mod types;

// Re-export main types
pub use error::{Error, Result};
pub use graph::DependencyGraph;
pub use label::{ModuleName, Version};
pub use parser::ModuleFile;
pub use registry::{Registry, RegistryChain, RegistryClient, RegistryClientConfig};
pub use resolver::{
    DirectDepsMode, ResolutionResult, ResolutionStats, Resolver, ResolverOptions, YankedBehavior,
    compare_versions,
};
pub use types::*;

/// Default Bazel Central Registry URL.
pub const DEFAULT_REGISTRY: &str = "https://bcr.bazel.build";

/// BCR GitHub mirror (fallback for certificate issues).
pub const DEFAULT_REGISTRY_MIRROR: &str =
    "https://raw.githubusercontent.com/bazelbuild/bazel-central-registry/main";
