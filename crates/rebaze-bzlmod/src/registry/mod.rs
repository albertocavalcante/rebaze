//! BCR registry client.
//!
//! This module provides HTTP clients for fetching module information from
//! Bazel Central Registry (BCR) and compatible registries.

mod client;

pub use client::{Registry, RegistryChain, RegistryClient, RegistryClientConfig};
