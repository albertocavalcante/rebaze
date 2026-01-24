# rebaze

Migrate from Gradle, CMake, and other build tools to Bazel.

## Status

**Early development** - not ready for production use.

**Bazel 9 only** - rebaze currently validates against Bazel 9.x.

## Supported Migrations

| Source | Status |
|--------|--------|
| Gradle | In progress |
| Maven | Planned |
| CMake | In progress |
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

# Use CMake File API data from an existing build dir
rebaze migrate /path/to/project --from cmake --cmake-build-dir /path/to/build

# Select a configuration and require File API data
rebaze migrate /path/to/project --from cmake --cmake-build-dir /path/to/build --cmake-config Debug --cmake-file-api-only

# Use bazelle (gazelle) for BUILD file generation (preferred default)
rebaze migrate /path/to/project --from cmake --build-generator bazelle --bazelle-root /path/to/bazelle

# Choose the generator explicitly (auto|native|bazelle)
rebaze migrate /path/to/project --from cmake --build-generator native

# Skip pre/post build validation
rebaze migrate /path/to/project --from cmake --unsafe-mode

# Validate generated files
rebaze validate /path/to/project

# Skip Bazel build validation
rebaze validate /path/to/project --unsafe-mode
```

`rebaze validate` runs `bazel build //...` and enforces Bazel 9 unless `--unsafe-mode` is set.

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
