//! Command extraction dispatch.
//!
//! Provides organized access to all CMake command extractors.

pub mod dependencies;
pub mod project_info;
pub mod subdirs;
pub mod targets;

pub use dependencies::{extract_package, extract_pkg_config};
pub use project_info::{extract_cmake_version, extract_project_info};
pub use subdirs::{extract_global_includes, extract_subdirectory};
pub use targets::{extract_compile_features, extract_executable, extract_library};
