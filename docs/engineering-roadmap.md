# Engineering roadmap

The next milestone is a reproducible native migration: small checked-in CMake
and Gradle projects migrate, build, and run their tests with Bazel 9 from a clean
checkout. The explicit discovery boundary is now implemented. Next align strategy
semantics, then prove the whole path. Additional source formats follow
those correctness gates.

This is the authoritative implementation sequence. The earlier
[file-decomposition proposal](refactoring-plan.md) is historical. Splitting a long
file can support a behavioral change; file length alone is not a completion gate.

## Current state

The project is between a vertical slice and hardening. The CLI, parsers, native
generators, typed Starlark rendering, and dual Cargo/Bazel builds exist. A clean
end-to-end migration contract still needs representative, required fixtures.

The Rust modernization and typed-generation changes landed in PRs
[#1](https://github.com/albertocavalcante/rebaze/pull/1) and
[#2](https://github.com/albertocavalcante/rebaze/pull/2). Their recorded evidence
includes 273 passing Cargo tests, strict Clippy, primary Bazel CI, independent
review, generated-file syntax tests, and source-helper checksum rejection tests.
These results describe that reviewed tree, not a promise that every future head
or input project passes.

| Evidence axis              | Assessment | What remains                                                                                                                      |
| -------------------------- | ---------- | --------------------------------------------------------------------------------------------------------------------------------- |
| Contract/correctness       | Partial    | Typed rendering/error propagation exist; strategy selection and semantic labels still need consistent validation.                 |
| Robustness/recovery        | Partial    | Path preflight and per-file atomic replacement exist; resource limits and complete-batch recovery are not established.            |
| Compatibility/conformance  | Partial    | Bazel 9 is the target; parser/syntax tests do not prove all source/rule/platform semantics.                                       |
| Consumer/integration proof | Partial    | Optional external CMake tests can return early when fixtures are absent; required native/JVM build-and-test fixtures are missing. |
| Delivery/repeatability     | Partial    | Hosted Cargo/Bazel CI exists; local shortcuts differ, docs checks are not PR-wide, and runmux activation lacks recorded proof.    |
| Documentation/usability    | Partial    | Contributor/review/architecture guides define the workflow; examples and support claims still need executable drift checks.       |

See the [architecture](architecture.md) for current behavior and boundaries.
Native rendering now consumes explicit discovery data. The next load-bearing
gap is agreement between strategy configuration, imports, aliases, and generated
dependency targets.

## Proposed implementation sequence

Keep each slice reviewable and independently mergeable. Every implementation PR
includes its regression, relevant Cargo/Bazel checks, compatibility impact, and
independent review of the final head.

| Order                   | Outcome                                                                 | Completion gate                                                                                                                                            |
| ----------------------- | ----------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 1                       | Completed: explicit discovery inputs and deterministic native rendering | Identical project/configuration/snapshot inputs produce identical path-content maps without running a process or accessing the network during generation.  |
| 2                       | One validated dependency strategy plan                                  | System, source, and BCR aliases/imports agree; unsupported or incomplete choices fail before writing; existing valid configuration has migration coverage. |
| 3                       | Required first-party migration fixtures                                 | Clean CI migrates native/JVM fixtures, builds them, runs their tests, and verifies expected behavior. Missing required inputs fail visibly.                |
| 4                       | Bounded inputs and defined failure behavior                             | Hostile/oversized inputs fail within documented limits, preserve recoverable state, and produce actionable diagnostics.                                    |
| Parallel delivery track | Repeatable contributor checks and verified runner/platform claims       | Local/CI checks agree; runner identity and head are recorded; native platform evidence matches every compatibility claim.                                  |

### 1. Completed: move host discovery out of rendering

`rebaze-bazel` now accepts an owned `DiscoverySnapshot` as an explicit CMake
rendering input. Snapshots retain exact requests and distinguish available data,
unavailability, and typed failures. They can be serialized and replayed. Missing,
mismatched, and failed observations are rejected before generated output writes.

`rebaze-core` owns pkg-config execution through a bounded adapter. Each distinct
request is probed once per pass. Argument validation, shell-escaped flag parsing,
concurrent capped stdout/stderr reads, and command deadlines have deterministic
and real-process tests. Output capture defaults to 256 KiB per stream and command
deadlines to five seconds; the adapter terminates/reaps the direct child on failure.

Generation has no host probes or log events. Complete file maps are compared for
fixed/replayed snapshots, including irrelevant observations. Discovery tests cover
missing tools, unsatisfied requests, unsuccessful queries, malformed data,
large legitimate output, output-limit boundaries, and hanging processes.

Discovery remains host-dependent and uses a trusted executable/environment. The
limits are per command/stream; whole-project budgets and descendant-process
containment are separate contracts. Native CMake pre-validation can still create
build state before discovery. See the [architecture](architecture.md) for details.

### 2. Build one typed strategy plan

[Generation](../crates/rebaze-bazel/src/lib.rs) currently creates a default
`ThirdPartyConfig`, while `MigrationConfig` contains separate strategy settings.
The MODULE, alias resolver, source targets, and system wrappers can disagree.

1. Parse/validate strategy values once. Define precedence for defaults,
   per-package overrides, BCR fallback, and explicit mappings. Preserve supported
   TOML spellings and test old configuration before changing public types.
2. Resolve normalized dependency identities, transitive edges, aliases, module
   imports, and source metadata into one plan consumed by every generated file.
   Detect unsupported selections and invalid source cycles explicitly.
3. Separate Meson, CMake, and Autotools options by build system. Validate required
   archive URL/checksum/output fields before emitting an active source target.
   Keep checksum verification before extraction and keep its negative test.
4. Start with system, source, and BCR cases. Treat other named strategies as
   unsupported until an explicit implementation and fixture prove them.

Tests must show that `strategy.default = "source"` is honored or rejected with an
actionable error; silently substituting the system default is not acceptable.
Generation failure must leave the original output untouched. Fetching/building
external source remains an explicit integration operation, outside pure rendering.

### 3. Prove real migrations with required fixtures

1. Check in small first-party fixtures: a CMake library/executable with headers
   and a Gradle JVM project. Pin their build inputs and expected observable results.
2. Exercise the actual CLI with `--build-generator native`, preserving a copy of
   the original fixture. Compare the original and migrated build/test behavior.
3. Extend coverage to a custom macro, a test target, and transitive system/source/BCR
   dependencies. Native CMake test generation is currently incomplete; add it as
   a separately reviewed implementation when its fixture requires it.
4. Keep networked Bazel/build oracles explicit and separate from offline unit tests.
   Record Bazel/rule/tool versions and the OS/architecture that actually ran.

The existing [external CMake tests](../crates/rebaze-bazel/tests/cmake_migration.rs)
currently return early without `test-fixtures/cmake-examples`. Make optional
corpus availability visible, and make required fixture absence fail. Corpus
reports need fixed repository revisions and explicit skips; a changing upstream
checkout is not reproducible compatibility evidence.

### 4. Harden inputs and recovery contracts

After the representative paths work, exercise semantic target/label validation,
configuration combinations, parser depth, file sizes, and dependency expansion.
Choose limits from documented supported workloads and test the boundaries.

Use deterministic malformed-input/property/fuzz regressions for repaired parser
failures. Test read/probe/write interruption at the owning effect boundary.
Per-file atomic writes currently allow an updated subset if a later write fails;
decide and document whether to add a recoverable batch journal or retain that
contract with explicit recovery guidance. Post-build failure also leaves output
for inspection.

## Parallel delivery work

These tasks support the milestone without widening its language support:

- Consolidate the commands in `justfile`, hooks, and CI around the same required
  gates. Preserve useful per-area checks and make missing tools/checks visible.
- Pin contributor tooling where it affects repeatability, including the Bun runtime,
  and run documentation checks on PRs. Add executable checks for first-run examples.
- Complete runmux through the [runner runbook](operations/ci-runners.md): add-only
  App grant, healthy engine/service, guarded pool change, canary, then complete CI.
  Record run URLs, runner names, and the reviewed head before changing compatibility claims.
- Keep x86-64, macOS, and Windows claims tied to native evidence. ARM64 runmux and
  cross-compilation do not replace those executions.
- Expand the standalone Bzlmod library against a named Bazel oracle/corpus before
  claiming complete override, multi-version, or lockfile compatibility.
- Before a release, reconcile declared licensing with packaged license/notice
  files, verify artifacts/checksums, and make release support match actual tests.

## Deferred capability and decisions

Maven migration, more source formats, complete multi-version Bzlmod resolution,
and broad file splitting follow the migration gates above. A new Starlark
serializer or full AST requires an executable case the current Serde models
cannot express; maintain one escaping implementation.

Decisions to settle in the owning implementation PRs: discovery/API migration,
strategy precedence, the first supported native/JVM fixture contracts, resource
limits, and the desired complete-batch write recovery contract. Runner activation
also needs working host credentials and infrastructure; workflow preparation
alone does not establish readiness.
