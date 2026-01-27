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
//! - **Override Support**: Handle all Bazel override types
//! - **Dependency Graph**: Query paths, explain version selections
//! - **Lockfile Support**: Read/write `MODULE.bazel.lock`
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

#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![deny(clippy::all)]
#![warn(clippy::pedantic)]

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
