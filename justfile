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

# Preview production docs build
docs-preview:
    cd docs && bun run build && bun run start

# Build docs for GitHub Pages (static export)
docs-gh-pages:
    cd docs && bun run build:gh-pages

# Test GitHub Pages build locally
docs-gh-pages-preview:
    cd docs && bun run build:gh-pages && bunx serve out

# Format docs code
docs-fmt:
    cd docs && bun run format

# Lint docs code
docs-lint:
    cd docs && bun run lint
