# rebaze development commands

# Default recipe
default:
    @just --list

# Build all targets
build:
    bazel build //...

# Run all tests
test:
    bazel test //...

# Format Rust code
fmt:
    cargo fmt

# Run clippy
lint:
    cargo clippy --all-targets

# Run the CLI
run *ARGS:
    bazel run //crates/rebaze-cli:rebaze -- {{ARGS}}

# === Documentation ===

# View docs (starts dev server)
docs:
    cd docs && bun run dev

# Install docs dependencies
docs-install:
    cd docs && bun install

# Start docs development server
docs-dev:
    cd docs && bun run dev

# Build docs for production
docs-build:
    cd docs && bun run build

# Preview production docs build locally
docs-preview:
    cd docs && bun run build && bun run preview

# Serve built docs (after docs-build)
docs-serve:
    cd docs && bunx serve dist

# Run full accessibility audit (requires docs server running)
docs-audit URL="http://localhost:4321/rebaze":
    ./tools/docs-audit.sh {{URL}}

# Quick accessibility check for CI
docs-audit-ci URL="http://localhost:4321/rebaze":
    ./tools/docs-audit-ci.sh {{URL}}
