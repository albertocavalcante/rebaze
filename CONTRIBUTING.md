# Contributing to rebaze

Use this guide to build rebaze, make a focused change, and submit evidence that
its behavior is correct. The [review guide](REVIEW.md) defines the review gate;
[AGENTS.md](AGENTS.md) adds instructions for coding agents.

## Set up a checkout

Clone the repository and create a branch for your change:

```bash
git clone https://github.com/albertocavalcante/rebaze.git
cd rebaze
git switch -c your-change
```

Install the tools required for the area you change:

| Area                       | Tools and version source                                                         |
| -------------------------- | -------------------------------------------------------------------------------- |
| Primary build              | Bazelisk, or Bazel from `.bazelversion`; a native C/C++ toolchain                |
| Rust development           | rustup and the toolchain/components in `rust-toolchain.toml`                     |
| Task shortcuts and hooks   | `just` and Lefthook; recipes/configuration live in `justfile` and `lefthook.yml` |
| Documentation site         | Bun; dependencies are locked in `docs/bun.lock`                                  |
| CMake migration validation | CMake and the input project's build dependencies                                 |
| Optional corpus tools      | Python/uv and the tools described in `tools/cmake_corpus/README.md`              |

Bazel is the primary build. Cargo is also checked in CI and supports local Rust
iteration. Bazel downloads its configured Rust toolchain; Cargo uses rustup.
Run the Cargo toolchain setup from the repository root:

```bash
rustup toolchain install --no-self-update
lefthook install
```

Hooks check selected files; they do not replace the complete checks below.
Bun, just, and Lefthook versions are not currently pinned by this repository.
When reporting a failure, include their versions if relevant.

## Build and run

```bash
bazel build //...
bazel run //crates/rebaze-cli:rebaze -- --help
```

For a faster Cargo iteration on one package:

```bash
cargo build --locked -p rebaze-cli
cargo run --locked -p rebaze-cli -- --help
cargo test --locked -p rebaze-starlark
```

Use `--build-generator native` when testing the native CMake generator. The
`auto` mode can select an installed bazelle binary and exercise a different path.
Run migrations in disposable fixtures or a dedicated branch: migration writes
project files and normal validation executes the original/generated build.

## Run the complete checks

These are the commands in [CI](.github/workflows/ci.yml), run from the repository
root. Keep a failing check visible in your pull request until it is fixed.

```bash
cargo fmt --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo build --workspace --locked
cargo test --workspace --locked
bazel build //...
bazel test //... --test_output=errors
```

`cargo fmt` applies Rust formatting. `just --list` shows convenience recipes;
the explicit CI commands above are the full check contract. In particular,
`just lint` currently has a narrower command than CI.

For a documentation-site change:

```bash
cd docs
bun install --frozen-lockfile
bun run build
```

The site build checks MDX and internal site links. Also check relative links in
repository Markdown; the site does not publish `CONTRIBUTING.md` or the roadmap.
See the [documentation guide](docs/README.md) for placement and authoring rules.

For other changed file types:

- Run buildifier on changed `BUILD.bazel`, `MODULE.bazel`, and `.bzl` files.
- Run actionlint on changed GitHub Actions workflows. Pin actions to verified
  full commit SHAs with the corresponding exact version tag on the same line.
- Run ShellCheck and shfmt on changed Bash/POSIX scripts, including extensionless
  scripts. Use native Zsh syntax checks for Zsh.
- Add meaningful Python type annotations and run Ruff formatting/linting plus
  BasedPyright on changed Python files. Existing hooks do not cover all of this.

Document the commands and results you actually obtained. Distinguish local
checks, hosted CI, cross-compilation, and native execution on each platform.

## Change the code

Use the [architecture guide](docs/architecture.md) to find the owning crate.
Follow Rust 2024 idioms and the workspace lint policy.

- Keep reusable logic in libraries and command-line orchestration in the CLI/core.
- Prefer pure transformations with explicit filesystem, process, and network boundaries.
- Use typed library errors (`thiserror`) and contextual errors in binaries (`anyhow`).
- Use `tracing` for diagnostics; reserve stdout for the command's intended output.
- Add a rationale immediately above each `#[allow(clippy::...)]`. Test-only
  `unwrap_used` allowances may omit the comment.
- Serialize project data through `rebaze-starlark` models. Keep static helper
  programs in `.bzl` templates; do not interpolate project text into source.
- Keep configuration intent distinct from observed host paths and package flags.

When changing dependencies, update Cargo manifests/`Cargo.lock` and the matching
Bazel deps. Refresh `MODULE.bazel.lock` through Bazel and review the generated diff.
Do not hide a stale lockfile by suppressing dependency checks.

To add a parser, add its Cargo workspace member, crate `BUILD.bazel`, and manifest
entry in `MODULE.bazel`. Wire it into core detection/migration explicitly and add
fixtures for the supported subset. Detection alone is not migration support.

## Native generation API

CMake rendering requires an explicit `DiscoverySnapshot`. Pass it to
`generate_from_cmake(project, &snapshot)` or
`generate_from_cmake_with_config(project, config, &snapshot)`. The former implicit
probe functions in `rebaze-bazel::third_party` have been removed. Third-party
render helpers also require a snapshot. `GenerationError` now distinguishes
render failures from discovery/snapshot failures; `PackageInfo` and `SystemLibrary`
replace the old probe-specific data types.

Application/core callers can obtain observations through
`rebaze_core::discovery::discover_packages`. Generation tests construct snapshots
explicitly; they must not depend on an installed pkg-config tool. Record
unavailability deliberately for fallback cases. Missing or stale requests and
failed probes must be tested as errors. The [architecture](docs/architecture.md)
describes limits, capture semantics, and the separation between rendering and discovery.

## Test behavior and failures

For a bug fix, provide a focused regression that fails on the base and passes
with the fix. Show the original reproduction when new APIs prevent the test from
compiling on the base. Refactors should preserve behavior and retain relevant
failure coverage.

Generation tests should cover serialized values, complete-file syntax, rule
semantics, and errors before writing. A parser accepting the output establishes
syntax; a real Bazel build/test establishes a separate compatibility claim.

The four CMake integration tests in
`crates/rebaze-bazel/tests/cmake_migration.rs` return early when their external
`test-fixtures/cmake-examples` checkout is absent. Green unit tests therefore do
not prove that corpus ran. Report fixture availability explicitly. The optional
[corpus tools](tools/cmake_corpus/README.md) download and execute external projects;
run them deliberately and record repository revisions, tool versions, and limits.

Keep ordinary unit tests independent of external services. A networked or
third-party build oracle belongs in an explicit integration check with pinned
inputs. A missing required fixture must fail that check visibly.

## Submit a pull request

1. Keep the change focused and the history linear. Use Conventional Commit
   subjects, such as `docs: explain contributor checks`, within 72 characters.
2. Explain the user-visible problem and resulting behavior. Include reproduction,
   validation, compatibility changes, and any failed or missing checks.
3. Update the appropriate docs with the implementation. Use the placement table
   in `docs/README.md` rather than copying procedures into several files.
4. Obtain the independent review described in `REVIEW.md`, address actionable
   findings, and obtain follow-up review after changes.
5. Squash merge only after required checks pass and the reviewed head is current.
   Verify the merged state and resulting target-branch commit.

Agent-created pull requests require an independent reviewer agent. Record the
reviewed SHA, findings, fixes, checks, and remaining limits in an expandable PR
section. An author self-review or green CI is not a substitute.

Never include credentials, local secret/config files, generated caches, or
unrelated work. Report infrastructure failures accurately; do not bypass enforced
checks. See [CI runner operations](docs/operations/ci-runners.md) for runmux gates.
