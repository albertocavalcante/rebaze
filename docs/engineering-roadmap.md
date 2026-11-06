# Engineering roadmap

The next milestone is a reproducible native migration: a supported Gradle or
CMake fixture migrates, builds, and runs its tests with Bazel 9 from a clean
checkout. Syntax-safe generation is a prerequisite; it does not establish that
all generated dependency strategies build successfully.

## Current evidence and boundaries

- Rust 1.99.1 and edition 2024 are pinned in Cargo and Bazel.
- `rebaze-starlark` provides Serde rule models, validated binding and call names,
  and structured file composition. Data is serialized; fixed helper programs
  are reviewed `.bzl` templates. Generation errors reach the CLI before writing.
- Parser tests cover whole generated files, hostile strings, load placement,
  and source metadata. These prove syntax and preservation of string values,
  not validity of every Bazel label or compatibility with every rule API.
- Source metadata has one configurable catalog with typed build systems.
  Dependency traversal terminates on cycles. GLib and PCRE2 archive pins were
  corrected against upstream/registry metadata and independently downloaded
  bytes; source helpers reject missing, malformed, or mismatching checksums.
- Native generation still probes `pkg-config` during rendering. Its result
  depends on the host and is not a declared reproducible input.
- CMake custom library and binary rules are emitted. Custom test rules remain
  reserved configuration; native CMake test generation is incomplete.
- Runmux routing and a manual canary are prepared. App access, pool changes,
  and an actual runner execution remain unverified while the Air is offline.

## Dependency order and completion gates

| Phase | Work | Completion gate |
|---|---|---|
| 1. Explicit generation inputs | Move `pkg-config` execution to a discovery layer. Pass a typed snapshot to a pure generation API. Preserve configured intent separately from observed paths and flags. | The same project, configuration, and snapshot yield byte-identical files without executing a process or accessing the network; probe failure is independently tested. |
| 2. Strategy semantics | Validate strategy names and per-package overrides. Make `strategy.default`, BCR fallback, generated aliases, and extension activation agree. Separate metadata options by Meson/CMake/Autotools. | Real system, source, and BCR fixtures build; unsupported strategies and incomplete metadata fail before writing. Existing non-system configuration must no longer be silently replaced by the system default. |
| 3. Consumer conformance | Add small checked-in JVM and native fixtures, including headers, transitive dependencies, tests, and a custom macro. Report absent optional external corpus inputs explicitly instead of silently skipping them. | A clean CI checkout migrates each fixture, runs `bazel build` and `bazel test`, and checks the result against the original build. Pin tool/rule versions and name the tested OS/architecture. |
| 4. Input and failure contracts | Validate semantic target/label names and configuration combinations. Bound parser depth, file sizes, and dependency expansion with documented limits. Exercise interrupted writes and hostile parser inputs. | Negative cases return actionable errors and preserve existing output; deterministic corpus/fuzz regressions cover each repaired failure. |
| 5. Delivery and compatibility | Complete runmux grant, pool allowlist, canary, and full ARM64 CI. Consolidate contributor checks. Expand resolver compatibility/override coverage against a named Bazel oracle before claiming full Bzlmod compatibility. | Record run URL, runner identity, and reviewed head. Required CI passes on the selected runner; native x86-64 claims retain x86-64 evidence. Resolver corpus documents supported and excluded behavior. |

## Runmux activation and rollback

1. On the Air, add only `rebaze` to the runmux App's selected repository lists;
   verify the plan adds one repository and removes none.
2. Add `albertocavalcante/rebaze` to `air-ci` scopes and `policy.allow_repos`
   through the guarded onboarding helper. Validate, drain, and restart without
   terminating active jobs. Keep the configuration backup.
3. Dispatch `runmux-canary.yml`. Require Linux ARM64, a nonzero UID, and an
   `rmx-air-` runner name; record its run URL and head SHA.
4. Set `CI_LINUX_RUNNER=air-ci` only after that evidence exists, then require the
   complete CI workflow to pass. A canary alone is not the build/test gate.
5. Remove `CI_LINUX_RUNNER` to restore hosted Linux routing. Do not change release
   platform jobs or widen App access to all repositories.

## Deferred capability

Maven migration, additional build systems, and complete multi-version Bzlmod
resolution follow the migration correctness gates. A new custom serializer or
full Starlark AST is justified only by an executable case the existing Serde
models cannot express; avoid maintaining two escaping implementations.
