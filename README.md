# rebaze

Migrate from Gradle, CMake, and other build tools to Bazel.

## Status

**Early development** - not ready for production use.

## Supported Migrations

| Source | Status |
|--------|--------|
| Gradle | In progress |
| Maven | Planned |
| CMake | Planned |
| Makefile | Planned |
| Cargo | Planned |

## Usage

```bash
# Analyze a project
rebaze analyze /path/to/project

# Migrate to Bazel (dry run)
rebaze migrate /path/to/project --dry-run

# Migrate to Bazel
rebaze migrate /path/to/project

# Validate generated files
rebaze validate /path/to/project
```

## Building

### With Bazel (primary)

```bash
bazel build //crates/rebaze-cli:rebaze
```

### With Cargo (for IDE support)

```bash
cargo build --release
```

## Development

```bash
# Run tests
bazel test //...

# Build release binary
bazel build --config=release //crates/rebaze-cli:rebaze

# Format code
cargo fmt

# Run clippy
cargo clippy
```

## License

Apache-2.0
