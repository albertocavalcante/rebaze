# Architecture

Use this guide to locate behavior, understand current boundaries, and assess the
impact of a change. The [engineering roadmap](engineering-roadmap.md) owns future
work; this page describes the implementation that exists now.

## Crate responsibilities

| Crate                   | Responsibility                                                                             |
| ----------------------- | ------------------------------------------------------------------------------------------ |
| `rebaze-cli`            | Clap arguments, configuration loading, logging, and command dispatch                       |
| `rebaze-core`           | Detection, migration orchestration, CMake/Bazel process execution, and generator selection |
| `rebaze-cmake`          | CMake syntax parsing, project extraction, and recursive source-file discovery              |
| `rebaze-cmake-file-api` | Adapt existing CMake File API replies into the shared CMake project model                  |
| `rebaze-gradle`         | Extract project information from supported Gradle Groovy/Kotlin forms                      |
| `rebaze-bazel`          | Map projects/configuration to generated files and write output files                       |
| `rebaze-starlark`       | Typed Serde rule models, identifiers/references, and Starlark file composition             |
| `rebaze-bcr`            | Built-in mappings from known C/C++ packages to Bazel Central Registry modules              |
| `rebaze-bzlmod`         | Standalone MODULE parsing, registry access, dependency resolution, and graph APIs          |

`rebaze-bzlmod` is a separate library in this workspace. The CLI migration path
does not currently use it as a complete Bazel resolver. Its lockfile API is
reserved, and several override/resolution modes return explicit errors.

## Native migration path

1. The CLI loads explicit or discovered configuration and calls `rebaze-core`.
2. Core selects the source system and generator. CMake uses parsed source files
   or an existing File API reply; Gradle uses its parser/project model.
3. For native CMake, core prepares one validated dependency plan, discovers only
   its selected system requests through `rebaze_core::discovery`, then renders
   that same preparation with the resulting `DiscoverySnapshot`.
   Gradle does not need this host snapshot. Both native generators return a map through
   `GenerationResult<HashMap<String, String>>`.
4. A dry run prints the map in path order. A writing migration passes the complete
   map to `write_files` before normal post-generation Bazel validation.

For CMake, normal migration first configures/builds the original project. The
default build directory is `.rebaze/cmake-build`, unless supplied explicitly.
After generation, normal CMake and Gradle migration run `bazel build //...`.
Neither step establishes that the original/generated test suites passed.

`--unsafe-mode` skips build validation. `--dry-run` skips writing and build
validation. It can still read project/configuration files and probe installed
packages; it is not a promise that all computation is independent of the host.

For CMake, `auto` prefers bazelle when available. The bazelle path executes an
external generator and writes scaffolding; native generation guarantees should
not be assumed to apply to that separate path.

## Data and effect boundaries

| Boundary           | Current contract and limit                                                                                                                                                                                                                                                |
| ------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Parsing/extraction | Syntax functions consume source data; file-oriented entry points also read project files. CMake extraction supports a subset of CMake semantics.                                                                                                                          |
| Configuration      | `MigrationConfig` owns versions, mappings, filters, custom rules, strategy settings, and the known source-package catalog. A typed plan validates managed pkg-config strategies and selected source graphs; other explicit CMake mappings retain their existing contract. |
| Host discovery     | Core runs pkg-config at an explicit boundary and supplies an owned snapshot. Rendering reads only that snapshot, project data, and configuration. Host discovery remains non-hermetic, but rendering no longer inspects the host.                                         |
| Starlark rendering | Models serialize values through `serde_starlark`. Validated identifiers, references, and calls distinguish generator syntax from project data.                                                                                                                            |
| Output writing     | All paths are preflighted. Each file uses a temporary file and atomic replacement. This is not a transaction across the complete output set.                                                                                                                              |
| Build validation   | Core executes CMake/Bazel and reports unsuccessful status. A failed post-generation build leaves generated files available for inspection.                                                                                                                                |

## Configuration intent

Configuration loading distinguishes an absent file from read/parse failures.
Both project convenience loaders return typed errors; a malformed file never
silently selects defaults. Core's loader accepts either flat native settings or
one `[bazel]` tree, rejects unknown fields recursively, and rejects mixed formats.
The target-specific `MigrationConfig` loader accepts flat settings only; a nested
envelope is an error there rather than an ignored section.

`ConfigProfile` identifies native CMake, native Gradle Java, or bazelle. Native
configuration support is checked before the original CMake build and again at the
public pure preparation boundary. Gradle/bazelle reject non-default overrides
instead of ignoring them. This does not make the complete command IO-free:
project detection and generator availability checks have their existing boundaries.

| Settings | Native CMake behavior |
| --- | --- |
| `versions.*` | Versions for the selected generated modules. |
| `mappings.system_libraries`, `plain_libraries`, `packages`, `implicit_system_libs`, `ignored_libraries` | Explicit link decisions after local/managed identity resolution. |
| `mappings.transitive_deps`, `known_packages.*` | Selected source graph, archive, outputs, and rule-specific options. |
| `strategy.*`, including BCR overrides/pins/skips | Managed provider selection and fallback under the existing plan. |
| `build.module_version`, `build.rules.*` | Module/rule composition; function and loaded binding names validate before tool execution. |
| `build.default_visibility` | Public/private visibility for root production/test targets; third-party wrapper visibility stays separate. |
| `build.default_c_standard`, `default_cxx_standard` | Defaults for present languages without detected standards; C-only flags also cover mixed C/C++ inputs. |
| `filters.*`, `build.header_patterns`, `mappings.system_linkopts` | Reserved profile data: non-default overrides fail explicitly. |

The [user reference](src/content/docs/reference/config.mdx) defines accepted values.
Configured label syntax/destination availability and complete source compiler
semantics still need their own validation. No configuration setting proves a
selected compiler supports its requested language standard.

## Discovery and rendering API

CMake callers supply discovery explicitly to `generate_from_cmake(project,
&snapshot)` or `generate_from_cmake_with_config(project, config, &snapshot)`.
These functions do not execute probes or emit log events. Generation diagnostics
such as unsupported language-standard warnings are returned in `.bazelrc`
comments; core owns logging of discovery outcomes and output counts.

`DiscoverySnapshot` is an owned, serializable map of exact package requests to
available data, explicit unavailability, or typed failures. A missing/mismatched
observation or a failed probe is a generation error. Callers can build snapshots
without running a tool, serialize them, and replay the same rendering inputs.
An empty snapshot is sufficient when the plan selects no system observations,
including source-only/BCR-only projects. `plan_cmake(project, config)` returns a
borrowed `CMakeGeneration`; discover its `system_modules()` and call its
`render(&snapshot)` to prepare once at the application boundary. The borrowed
project/config cannot change underneath that preparation.

`rebaze_core::discovery::discover_packages` is the explicit host adapter. It probes
each distinct request once per pass, preserving version constraints. The adapter
uses argument vectors, shell-escaped flag parsing, concurrent capped stdout/stderr
capture, and a deadline that includes process exit and pipe draining. Defaults are
five seconds per command and 256 KiB per stream. Library callers can configure
limits within the validated range: up to 300 seconds and 16 MiB per stream.
Requests have a 64 KiB argument budget and reject option/control-character input.

Missing tools and unsatisfied requests retain the fallback/stub behavior, with
core warnings. Other unsuccessful queries, invalid UTF-8/flags/prefixes, timeout,
and capture overflow are typed failures; generation rejects them before writing
Bazel outputs. A normal CMake migration can already have run original-build
validation before discovery, so this is not a no-filesystem-effects guarantee
for the complete command.

The synchronous adapter uses a dedicated runtime thread and works inside an
existing async runtime. Cancellation closes capture handles and terminates/reaps
the direct child. The executable and environment remain trusted host inputs;
this is not a process sandbox or a guarantee of terminating descendant processes.
Limits are per command/stream, not a whole-project work budget.

System wrappers symlink observed include roots, including outside-prefix roots,
propagate joined `-D` definitions, and retain ordered linker arguments with Make
and Bourne escaping. Other compiler flags remain snapshot data and require
manual review. Three explicit build oracles cover representative system, source,
and BCR cases; they do not establish complete pkg-config flag conformance.

## Starlark generation

`File` combines and hoists loads before rule calls while preserving body order.
Typed models cover native/JVM rules, extensions, module dependencies, collections,
and assignments. Invalid binding/call names or serialization errors propagate
through generation to core; they are not converted into successful output comments.

Fixed helper programs live in `crates/rebaze-bazel/src/templates/*.bzl`. Dynamic
configuration is assigned through serialized tables. The system helper uses
`json.encode` when it constructs a nested BUILD file from dynamic values.
Keep the boundary between trusted helper code and project data explicit.

Source-based CMake generation supports unconditional literal
`add_test(NAME name COMMAND executable_target args...)` registrations, including
subdirectories, with an unconditional root `enable_testing()`. It retains each
binary and emits a separate `ctest_<name>` target using the configured test rule
(default `cc_test`). Names normalize hyphens to underscores; invalid names,
collisions, unknown/ambiguous executables, and unsupported registrations fail
during planning before discovery or generated output writes. Arguments remain
serialized data and are escaped for Bazel's Make expansion and argv tokenization.

Conditional/scoped test declarations and directory inclusion, dynamic property
selection, variables/generator expressions, legacy
syntax, properties, configuration selection, explicit working directories, and
list expansion, launchers and cross-compilation emulators are rejected. This subset targets self-contained tests: CTest's
build-directory working environment and Bazel's sandbox/runfiles environment are
different. Test data files and CMake File API test import are not implemented.
The explicit first-party CLI oracle checks both successful and failing tests;
normal CLI migration validation continues to build without running test suites.

One configurable `KnownPackageInfo` catalog supplies source archive metadata and
build targets. `SourceBuildSystem` distinguishes Meson, CMake, and Autotools.
The plan unions catalog dependencies with configured transitive edges, rejects
cycles, and limits source graphs to 64 levels and 1024 packages. It accepts at
most 1024 root aliases and 256 packages per alias. These are planning bounds, not
a total execution budget. Meson options become a typed map; CMake/Autotools
options reach their rule-specific fields. Only active targets/archives/imports
are emitted; generated `config.bzl` records resolved intent rather than running
a second strategy resolver.

Source repositories require a 64-character hexadecimal SHA-256 and use Bazel's
checksum verification before extraction. Unknown packages need configuration;
source metadata and extension activation do not constitute universal build support.

Native preparation rejects unresolved required link dependencies before host
discovery or generated output writes. Mapping APIs return `DependencyMapping`:
mapped labels, explicit omissions with `OmissionReason`, or unresolved input.
Unknown dependencies no longer become TODO comments that rendering drops.
Configured comment strings are rejected as mappings. Empty mapping values are
explicit omissions; configured ignores, implicit system libraries, and the
existing built-in package policy remain supported. Diagnostics identify the
source target and dependency. Preparation uses one output-name table for libraries, executables, and generated
tests. Source extraction preserves alias declarations, including duplicates and child
directories, in `Vec<TargetAlias>`; preparation derives the validated lookup map.
Aliases cannot shadow prepared managed identities, and undeclared case variants
of the pkg-config namespace cannot manufacture third-party labels. Production
names accept ASCII letters, digits, underscores, hyphens, dots,
and plus signs (except `.` and `..`); hyphens normalize to underscores, and
collisions fail explicitly. Aliases resolve iteratively to exact local identities
or configured external mappings; cycles, unmapped endpoints and attempts to link
executables fail before discovery. Namespace suffixes never select unrelated
local targets. `PkgConfig::` links require a matching prepared request. Each
production target receives only its selected link dependencies, resolved once in
the immutable preparation. Configured labels still need separate syntax and
destination-availability validation; complete native link semantics remain R10
work.

## Gradle Java generation

Gradle input now has one `GradleModule` per declared project, including implicit
parents of nested `include` paths. Discovery reads settings/build files and
conventional `src/main/java`, `src/test/java` and main/test resource trees. It
rejects ambiguous Groovy/Kotlin build files and source symlinks, including symlinked
ancestors. Undeclared directories are not treated as projects. `GradleError`
retains source/IO diagnostics. The lexer/parser and Java-header classifier consume
source text; generation consumes the resulting owned data without IO.

`Dependency` distinguishes literal Maven coordinates from project references.
Generation validates paths, plugins, fixed Maven versions, label collisions and
production project cycles before returning files. It partitions implementation,
API exports, runtime-only, test-implementation and test-runtime dependencies.
Project references must be absolute colon-prefixed paths. Generated libraries
are `//path:lib`; tests are `//path:test_<qualified_class_with_underscores>`.
Source/resource lists are explicit observations; regenerate when files change.
No empty placeholder tests or test dependencies are emitted on production rules.

The supported test subset is conventional public top-level JUnit 4 classes with
direct `org.junit.Test` annotations, package/path agreement and an explicit JUnit
4 dependency. Helpers compile with test sources but do not become test targets.
Inheritance, abstract/ignored/custom-runner classes, JUnit 3/5, Unicode escapes
and text blocks require broader Java/Gradle evaluation and are rejected. This is
a source subset, not a complete Java parser or Gradle interpreter. Built-in
`java`, `java-library` and aggregator `base` plugins are supported; other plugins,
custom blocks, source sets, compiler settings, version catalogs, exclusions,
compile-only scopes and dynamic coordinates are unsupported.

Generated files target Bazel 9, Java 21, rules_java 9.9.0 and
rules_jvm_external 6.10. The project's `rebaze_maven` namespace stays distinct
from Maven repositories contributed by toolchain modules. A fixed Java-only
`aar_import` adapter avoids loading Android rule implementations when importing
JARs; it explicitly fails AAR dependencies. It uses a canonical main-repository
label, so it also loads correctly from the generated external Maven repository.

An existing root `maven_install.json` is explicit input data and is preserved in
the output map. Normal CLI build validation requires this reviewed lock for
Maven dependencies. Preview and `--unsafe-mode` can prepare an unpinned module
without executing builds; pin/review explicitly before normal migration. The
first-party fixture additionally has strict Gradle SHA-256 verification metadata.
Its explicit oracle compares real original/generated tests and deliberately
failing assertions. It establishes the tested Java/JUnit subset and platforms,
not arbitrary Gradle/Kotlin/Android or complete dependency-resolution equivalence.

## Failure and compatibility evidence

Native generation completes before output writing. The writer rejects traversal,
conflicting output paths, and symlink destinations during preflight. It assumes
callers prevent concurrent destination-directory changes. An I/O failure after
some replacements can leave a partial batch; preserve the original project in
version control or a disposable workspace.

Parser-oracle tests check complete generated syntax and string-value preservation.
Other tests cover configured rules, source metadata, dependency traversal, and
generation errors before writing. Optional external CMake fixtures can be absent
without failing their current tests. Consult [CONTRIBUTING.md](../CONTRIBUTING.md)
for check commands and the [roadmap](engineering-roadmap.md) for missing
end-to-end, resource-limit, and compatibility gates. Non-pkg explicit imported
target mappings and complete per-target CMake dependency semantics remain outside
the managed plan. Foreign build metadata is not universal compatibility proof.
