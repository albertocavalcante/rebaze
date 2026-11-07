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
CI pins and verifies Bun in `.github/scripts/setup-bun.sh`. Local Bun, just,
and Lefthook versions are not enforced. Include their versions when reporting
relevant local failures.

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

CI runs the full Rust/Bazel and migration commands for code, build inputs, CI
scripts, and unrecognized paths. Known repository-only Markdown changes run
lightweight routing, structure, and local inline-link checks instead. Published
site inputs run the site build on PRs and the deployment workflow on main.
Mixed changes select every relevant check. Manual CI dispatch selects everything.

`CI Result` checks the selected job outcomes, including intentional skips. Failed
routing or cancelled/failed required jobs cannot produce a successful result.
The routing suite includes Markdown checks using two small hash-pinned packages:

```bash
python3 -m venv .rebaze/ci-python
.rebaze/ci-python/bin/python -m pip install --require-hashes --only-binary=:all: -r .github/requirements-docs.txt
.rebaze/ci-python/bin/python -m unittest discover -s .github/scripts -p 'test_ci_routing.py'
.rebaze/ci-python/bin/python .github/scripts/ci_routing.py check-docs
```

The Markdown check parses CommonMark with tables. It checks top-level heading
structure and local link/image destinations, including reference links, while
ignoring literal/code examples. External URLs, anchors, and Astro MDX/site links
need their own checks.

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

Use `plan_cmake(project, config)` to prepare native CMake generation. The borrowed
`CMakeGeneration` validates strategy intent once. Pass only `system_modules()` to
`rebaze_core::discovery::discover_packages`, then call `render(&snapshot)` on the
same preparation. This keeps discovery and output selection consistent.

The convenience functions `generate_from_cmake(project, &snapshot)` and
`generate_from_cmake_with_config(project, config, &snapshot)` still work; they
prepare internally. Source-only/BCR-only plans accept an empty snapshot. System
requests, including selected source transitives, need exact observations.
`GenerationError` distinguishes planning, rendering, and discovery failures.

Native preparation also returns `GenerationError::NativeLink` for unresolved
required dependencies, identifying the source target and link. Add the appropriate
mapping in `rebaze.toml`, or use an intentional ignore only when that dependency
really needs no generated edge. Mapping methods and the `filters` convenience
wrappers now return `DependencyMapping` rather than `Option<String>`; match
`Mapped`, `Omitted(reason)`, and `Unresolved` explicitly. Unknown links are never
successful TODO comments. Empty configured mapping values remain explicit omissions.
Preparation also rejects output-name collisions and invalid/cyclic aliases.
Namespace-qualified links need an exact alias, a prepared pkg-config request,
or a configured mapping; they never match a local library by suffix. Only a
target's selected dependencies become generated edges. `CMakeProject.aliases`
is now a `Vec<TargetAlias>` of observed declarations rather than a map; retain
all declarations so preparation can reject duplicates. Recursive source extraction
preserves child-directory aliases.

`ThirdPartyConfig`, `DepsStrategy`, and direct third-party rendering helpers have
been removed. They could disagree with `MigrationConfig` and the shared plan.
Use `MigrationConfig.strategy` and the prepared API instead. Raw TOML spellings
remain strings at the configuration boundary; the plan accepts only implemented
`DependencyStrategy` values. Generated `config.bzl` is inspection data; change
`rebaze.toml` and regenerate to change providers.

Generation tests construct snapshots explicitly and must not rely on installed
pkg-config tools. Record unavailability deliberately; test missing/stale requests
and failed probes as errors. The [architecture](docs/architecture.md) describes
capture semantics, limits, and remaining compiler-flag/mapping gaps.

### Explicit generated dependency builds

On Unix with Bazel 9, a C toolchain, CMake, Ninja, tar, shasum, and network access,
run the three required cases for a dependency-strategy change:

```bash
cargo test -p rebaze-bazel --locked dependency_build_tests -- --ignored --test-threads=1
```

This command explicitly selects ignored networked oracles; ordinary Cargo/Bazel
unit tests do not execute them. Missing tools, downloads, or fixture inputs fail
the selected tests. The fixtures build/run system, first-party CMake-source, and
BCR-zlib applications. The system case covers outside-prefix headers, joined
compile definitions, spaced/quoted paths, and ordered linker flags. The source
case verifies a CMake option changes the program's result. It computes a checksum
for its own first-party archive rather than trusting an external artifact.

`REBAZE_BAZEL_TEST_ROOT` can name an absolute cache directory for these tests.
They ignore system/home Bazel rc files and pass only PATH/HOME to fixture
processes. Their results establish the tested host's compatibility, not native
Windows execution or complete Meson/Autotools/GLib support. Record which oracles
actually ran, separately from standard CI and optional corpus tests.

### Required CMake CLI migration

On Unix with Bazel 9, CMake/CTest, Make, a C compiler, and network access:

```bash
cargo build --locked -p rebaze-cli
bash .github/scripts/test-cmake-migration.sh target/debug/rebaze
```

CI explicitly runs this oracle after the ordinary Cargo tests. It bootstraps
verified Bazel and CMake 3.31.10 archives on the minimal Linux runner. Ordinary
library tests never download these tools or execute the oracle implicitly.

The checked-in first-party fixture contains a library, public header, executable,
and a test registered in a subdirectory. An alias declared in that child directory
is linked by the parent executable. The harness copies it into disposable
original/migration directories, checks preview behavior, invokes the real CLI
with normal build validation, and compares CTest/Bazel test and executable results.
It then changes a library result: both builds must succeed and both test suites
must fail. Missing tools/files and missing tests are failures. The literal argv
case includes spaces, a single quote, and a dollar sign.

The harness clears unrelated environment variables for the CLI/Bazel, ignores
system/home Bazel rc files, and uses a separate output root. Optional
`REBAZE_CMAKE_TEST_CACHE` names an absolute reusable Bazel output root; do not share
it between simultaneous runs. It does not establish Gradle, File API test import,
arbitrary CMake logic, external corpus, or native Windows compatibility. The CLI's
normal migration validation still builds only; the harness explicitly runs tests.

### Required Gradle/Java CLI migration

With JDK 21, Gradle 9.8.2, Bazel 9 and network access:

```bash
cargo build --locked -p rebaze-cli
JAVA_HOME=/path/to/jdk-21 bash .github/scripts/test-java-migration.sh target/debug/rebaze
```

Replace the JDK path with your installation. CI bootstraps checksum-verified
Temurin 21.0.13+9 and Gradle 9.8.2 on Linux ARM64/x86-64, then runs this required
oracle separately from ordinary unit tests. The fixture has two Java projects,
API/project/runtime/test dependencies, main/test resources, two JUnit 4 classes
(including one without a `Test` filename suffix), and a non-test helper.

The harness runs original Gradle tests, normal native CLI migration and Bazel
build/tests in disposable copies. It checks two discovered classes, preview
behavior, input preservation, and rejected framework/abstract-class/symlink
changes preserving an existing BUILD. Changing a main resource must leave builds
successful while both test suites fail. Missing files/tools/tests fail visibly.
System/home Bazel rc files, Gradle user init scripts and unrelated environment
are excluded. `REBAZE_JAVA_TEST_CACHE` may name an absolute reusable Bazel output
root; never share it between concurrent runs.

The fixture pins JARs in `maven_install.json` and Gradle artifacts/metadata in
`gradle/verification-metadata.xml`. Normal CLI build validation requires an
existing reviewed Maven lock when Maven dependencies are present. To prepare a
new lock deliberately in a disposable copy, generate with `--unsafe-mode` (no
builds), run `bazel run @rebaze_maven//:pin`, review the downloaded artifacts and
reported JSON, save that JSON as `maven_install.json`, and migrate normally again.
After pinning, update it with `REPIN=1 bazel run @rebaze_maven//:pin`. Review the
lock diff with dependency edits. Refresh Gradle verification metadata explicitly
with `gradle --write-verification-metadata sha256 test`, including a cold-cache
run; compare artifacts/checksums with Maven Central before accepting changes.
Never disable verification to make a missing checksum pass.

Source extraction supports literal Java metadata in Groovy/Kotlin build files;
it does not execute Gradle or evaluate arbitrary DSL. Unsupported plugins,
custom settings/build logic, compiler/source-set customization, relative project
dependencies, compile-only configurations, dynamic versions and non-JUnit-4 test
semantics fail visibly. See [architecture](docs/architecture.md) for the exact
source/test subset. Normal CLI validation remains build-only; the harness runs
Gradle/Bazel tests explicitly.

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
