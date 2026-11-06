# rebaze

Migrate from Gradle, CMake, and other build tools to Bazel.

## Status

**Early development** - not ready for production use.

**Bazel 9 only** - rebaze currently validates against Bazel 9.x.

## Supported Migrations

| Source | Status |
|--------|--------|
| CMake | ✅ Supported |
| Gradle | ✅ Supported |
| Maven | 🔜 Planned |
| Makefile | 🔜 Planned |
| Cargo | 🔜 Planned |

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
cargo clippy --workspace --all-targets --locked -- -D warnings
```

## Starlark generation

`rebaze-starlark` owns typed rule models and file assembly. Generators serialize
project values, paths, labels, dictionaries, and lists through Serde; they do not
interpolate them into Starlark source. `Identifier`, `Reference`, and `Call` keep
variable bindings, references, and call names separate from string values.

`File` hoists and combines loads when documents are composed. Fixed helper
programs live in `crates/rebaze-bazel/src/templates/*.bzl`; dynamic configuration
is assigned through serialized tables. Generation returns `Result`, and the CLI
finishes generating the whole file set before writing it.

Tests parse complete generated files with `starlark_syntax`, including quotes,
backslashes, Unicode, control characters, and attempted statement injection.
These checks establish syntax and data preservation; they do not prove that every
migration builds against every dependency or platform.

The [engineering roadmap](docs/engineering-roadmap.md) records remaining
correctness and integration gates.

## CI runners

Linux CI uses the `CI_LINUX_RUNNER` repository variable, defaulting to
`ubuntu-24.04`. To onboard runmux, add `albertocavalcante/rebaze` to the runmux App's
selected repositories and the Air's `air-ci` pool scopes and allowlist, then run
the manually dispatched **Runmux Canary** workflow. It checks Linux ARM64,
non-root execution, and an `rmx-air-` runner identity without checking out code.

Only after that canary succeeds, set `CI_LINUX_RUNNER` to `air-ci` and run the
complete CI workflow. Remove the variable to return to hosted Linux runners.
The Air must be online and its runner policy must allow pull requests. ARM64
results do not establish x86-64 compatibility; release platform jobs remain on
their native hosted runners.

## License

Apache-2.0
