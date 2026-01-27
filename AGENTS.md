# Agent Instructions for rebaze

## Project Overview

rebaze is a CLI tool that migrates projects from other build systems (Gradle, Maven, CMake) to Bazel.

## Architecture

```
crates/
├── rebaze-cli/      # CLI entry point (clap-based)
├── rebaze-core/     # Orchestration and shared models
├── rebaze-gradle/   # Gradle build file parser
├── rebaze-bazel/    # Bazel file generator
```

## Build System

- **Primary**: Bazel 9 with rules_rust
- **Secondary**: Cargo (for IDE compatibility)

## Key Commands

```bash
# Build
bazel build //...

# Test
bazel test //...

# Run CLI
bazel run //crates/rebaze-cli:rebaze -- analyze .

# Format
cargo fmt

# Lint
cargo clippy
```

## Code Style

- Follow Rust 2024 edition idioms
- Use `anyhow` for error handling in binaries
- Use `thiserror` for library error types
- Prefer `tracing` over `println!` for logging

## Clippy Lints

- All `#[allow(clippy::...)]` directives MUST have a rationale comment explaining why
- Place the comment on the line immediately before the `#[allow(...)]` attribute
- Example:
  ```rust
  // BUILD file generation needs sequential sections (loads, package, targets) that can't be easily split
  #[allow(clippy::too_many_lines)]
  pub fn generate_root_build(...) { ... }
  ```
- For test modules, `#[allow(clippy::unwrap_used)]` is acceptable without comment since tests intentionally panic on failure

## Adding a New Parser

1. Create `crates/rebaze-{name}/` with `Cargo.toml`, `BUILD.bazel`, `src/lib.rs`
2. Add to workspace in root `Cargo.toml`
3. Add to `MODULE.bazel` manifests list
4. Implement the parser following `rebaze-gradle` as reference
5. Add integration in `rebaze-core`

## Testing

- Unit tests: `#[cfg(test)]` modules in each file
- Integration tests: `tests/` directory with fixture projects
