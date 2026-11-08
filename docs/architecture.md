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
| `rebaze-input`          | Counted text reads, shared IO budgets and explicit project-root capabilities                |
| `rebaze-starlark`       | Typed Serde rule models, identifiers/references, and Starlark file composition             |
| `rebaze-bcr`            | Built-in mappings from known C/C++ packages to Bazel Central Registry modules              |
| `rebaze-bzlmod`         | Standalone MODULE parsing, registry access, dependency resolution, and graph APIs          |

`rebaze-bzlmod` is a separate library in this workspace. The CLI migration path
does not currently use it as a complete Bazel resolver. Its lockfile API is
reserved, and several override/resolution modes return explicit errors.

`rebaze-input` is a leaf IO library; `rebaze-cmake` has a versioned dependency on
it. A registry release must make that input version available before publishing
CMake. Local archive-consumer checks use an explicit local patch for this
unpublished dependency; they do not establish crates.io readiness for all
workspace libraries or authorize publication.

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
Both file loaders cap TOML at 64 KiB, require a regular UTF-8 file and reject
configuration-file symlinks, including valid links. The explicitly selected parent
is opened once for the read; this policy does not reject symlinks in its ambient
root selection or promise a snapshot across separate configuration loads.
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

## CMake source evaluation

Syntax parsing (`parse_source`) remains a pure AST operation. Source extraction
records `SourceIssue` values in `CMakeProject.requires_evaluation`; native
`plan_cmake` rejects these before discovery or generated output writes. The normal
native CLI prepares source-only input before configuring/building the original
project. Explicit configured File API input follows its separate evaluated-data
path: normal validation refreshes that build before its replies are captured;
unsupported source never causes an implicit configure/evaluate fallback.

Borrowed-source parsing defaults to 1 MiB and 32 active recursive grammar scopes.
`parse_source_with_limits` accepts a validated `ParseLimits` profile up to 4 MiB
and 128 scopes; zero or larger settings are errors. Parenthesis groups, nested
variable grammar and nested generator expressions share that scope budget.
Literal delimiter text in quotes, brackets and comments does not count as
recursive syntax. Scopes are released on success, failure and backtracking;
exhaustion returns an explicit diagnostic instead of a partial AST. Each parse
owns its state, without filesystem, SDK or environment access.

These are parser-allocation/recursion bounds for already borrowed text. Recursive
CMake source capture also uses `rebaze-input`: one opened root, 1 MiB per file,
64 MiB aggregate consumed bytes, 4,096 file attempts, 65,536 logical operations
and 64 relative path components by default. `extract_project_from_path_with_limits`
accepts validated `ReadLimits`; increasing its IO allowance leaves syntax limits
unchanged. File and operation attempts are charged on failure; bytes consumed by
invalid UTF-8 and oversize proof reads remain charged. Reads check the opened file
before and after reading; detected length/modification changes fail explicitly.

Relative CMake symlinks may resolve beneath the original root, including internal
`../` link targets. Absolute link targets, escapes, lexical parent components and
special files are rejected. Canonical relative paths detect repeated directories;
they are not inode identities. On Unix, root renames do not redirect the opened
reader to a replacement directory. Windows capability handles instead prevent
renaming the live root; native Windows behavior has not been executed here.
This is neither an immutable snapshot nor proof of
hard-link/mount provenance. Nonblocking open avoids a FIFO replacement hang; it
does not give regular/network files a deadline. Gradle, File API, lock/network
readers, directory enumeration, expansion and output budgets remain R07–R09 work.
This does not claim a process deadline or a bound on all parser work. Chumsky's
existing stack-growth support remains enabled; a reproduced stack overflow is
not the basis for these limits.

Supported ordinary `set` assignments and `list(APPEND/PREPEND ...)` changes are
processed in statement order, including target sources and properties. Each
literal `add_subdirectory(relative/path)` captures the parent's variable context
at that call. Child mutations are isolated; later parent assignments cannot
rewrite an earlier child. Required directories/listfiles must exist, and repeated
or cyclic source-directory visits are errors. AST-only extraction cannot read
children: its model records that incompleteness; use
`extract_project_from_path` for recursive source extraction.

The native source profile rejects conditionals, loops, function/macro definitions,
blocks, cache/environment/parent-scope assignments, unsupported list operations,
unknown commands, dynamic metadata names, generator expressions, nested/undefined
variables, semicolon-list syntax and quoted variable expansion. It does not
extract scoped bodies as unconditional targets. Tests retain their separate
literal argv validation, including empty/quoted/bracket arguments. `include`
currently accepts only literal `FindPkgConfig` without a custom module path.
Custom includes, module/toolchain hooks, every `find_package` call and generated
source commands need explicit evaluated evidence: package modules/config scripts
can mutate arbitrary source variables. Mutating `CMAKE_*` variables is allowed
only through ordinary `set` for the two scalar language standards and the two
separately validated test-launcher settings; other mutations (including
project-specific hooks and all `list` mutations) need evaluated evidence. `BUILD_SHARED_LIBS` mutation also needs evaluation.
Package metadata is retained for analysis,
but does not authorize native source migration. Other metadata commands such as
`project`, `pkg_check_modules` and `target_compile_features` require
literal arguments throughout; their extractors do not evaluate variable values.
Built-in path substitution is
limited to `CMAKE_CURRENT_SOURCE_DIR` in include-directory commands.

Target property commands require a preceding target in the same directory.
Cross-directory target mutation, inherited directory include settings and differing
per-target/directory language standards are rejected when the current model cannot
represent their scope. Standards are captured when targets are created; a later
assignment does not retroactively change them. Complete compiler features,
usage-requirement propagation and flag ordering remain R10 work. This static
profile is deliberately narrower than CMake and is not a general interpreter.

The required first-party CMake oracle builds and tests both original and generated
projects. It includes an early target between two assignments and child variable
inheritance/isolation; missing source or leaked state makes the build fail.
A separate stale-reply case changes a configured target before normal migration
and requires the refreshed File API target in output. Independent native execution
establishes these cases, not arbitrary CMake
compatibility or a complete external corpus pass.

## Ordered output intent and lowering reports

`plan_cmake(...).render_bundle(&snapshot)` and `generate_bundle(&gradle_project)`
return `GenerationOutput`: an immutable ordered `OutputBundle` plus a
`LoweringReport`. Construction and rendering perform no filesystem, process or
network IO and emit no logs. A failed constructor/planning/render returns an error
without an applicable bundle. `GenerationError::diagnostic()` supplies a stable
category and a source/target/configuration origin where the model has one; missing
origins remain explicitly unknown rather than fabricated locations.

`OutputPath` uses a conservative ASCII component grammar, excluding traversal,
empty components, Windows-invalid characters, reserved device stems and trailing
dots/spaces. Bundles reject duplicate paths, ASCII case aliases and file/ancestor
conflicts under that same case folding. Their paths and bytes iterate in lexical
order. This narrows accepted output names (including Gradle module directories);
Unicode remains supported in serialized content and input data. The filename rules
are a deliberate portable subset, not a claim to model every filesystem alias:
see [Windows filename conventions](https://learn.microsoft.com/en-us/windows/win32/fileio/naming-a-file).
Actual destination state and races are application concerns, outside pure intent.

The lowering report currently records native CMake direct input-link mappings and
explicit omission reasons. It does not enumerate provider transitives; an empty
JVM report does not claim equivalent semantics or complete coverage. A successful
lowering report is neither a successful write nor a successful build/test result.
Core stage results and verification evidence use the separate report boundary
below; lowering does not promote either kind of evidence.

`generate`, `generate_from_cmake*` and `CMakeGeneration::render` retain HashMap
compatibility wrappers with identical successful bytes; their iteration order is
not guaranteed. `write_files` validates through an owned bundle and delegates to
`write_bundle`; use the bundle API to avoid cloning a legacy map. Empty/invalid
legacy batches now fail before writes. Application retains the existing per-file
replacement contract, partial-batch failure behavior and concurrency limitations;
journaled batch integration remains R06 work. The opt-in Linux/macOS
[`apply` API](guarded-application.md) implements guarded single-file and
existing-parent batch application
with explicit baseline/metadata intent, a cooperative lock, durable journal,
no-clobber install/restoration and retained originals. Core does not use it yet;
the metadata contract excludes ACLs/xattrs and needs an explicit integration policy.
Batch application stages every output before moving destinations, binds ordered
per-file intent to one transaction, and reconciles actual state during forward
recovery. Durable stop/restoration intent prevents reapplication. Receipts report
current per-output evidence, including preserved user deletion and mixed terminal
conflict; no whole-batch undo or observer-atomic update is promised.

## Core stages, previews and build evidence

`rebaze_core::migrate_with_report` and `validate_with_report` return structured
`MigrationReport` data. Failures retain the completed/failed stages and original
error in `MigrationFailure`; a failed application can leave a partial batch, so
reported output paths describe intent rather than recovery or successful writes.
Legacy `migrate`/`validate` keep unit-returning execution wrappers but discard report
data. They no longer print previews; callers needing preview data use the report
APIs. `rebaze-cli` owns preview presentation and sends its tracing to stderr.
Native preview source text goes to stdout through a fallible writer.

Reports distinguish workspace inspection, build-system/generator selection,
configuration support, capture, preparation, discovery, rendering, application,
external generation and build stages. A passed discovery stage means capture
completed, not that every package is available: request/policy and snapshot
outcomes remain explicit. Pkg-config limits are currently five seconds per command
and 256 KiB per stream; this is not a whole-operation budget. Build/external process
bounds remain R08 work.

Source evidence records actual CMake syntax, configured File API or Gradle capture.
Fallback to source is reported as source capture. Configured capture rejects a
source root different from the selected project. `FileApiCapture` retains the
configuration selected from the codemodel separately from requested configuration
intent, using a named/unnamed observed value. Missing capture is distinct from an
observed unnamed configuration. The existing first-configuration default selection
policy is unchanged.
Normal File API validation refreshes before capture; preview/unsafe mode reads
existing replies. These fields do not claim configuration or compiler equivalence.

`ClaimResults` starts with nonempty required build claims in core. Missing results,
explicit unknown results and an empty required set aggregate to `Unknown`; a
required failure takes precedence. CMake migration requires original and generated
build results; Gradle migration and workspace validation require the generated
build. These claims record command success only, not tests or semantic equivalence.
Preview and `--unsafe-mode` leave unexecuted claims unknown. CLI failures remain
nonzero, and missing required evidence without an explicit validation skip is an
error; deliberate skips never print a verification pass.

Preview reads inputs and can perform the existing bounded pkg-config discovery.
It executes no builds or Bazelle version probes and applies no generated files.
Automatic tool selection inspects PATH candidates (including Unix execute bits),
not command success. Actual execution is checked only in the external stage.
Bazelle preview returns planned argument vectors and conditional scaffolding:
create if absent, preserving existing files and requiring an existing Bazel 9
version. It supplies no complete validated bundle. Normal Bazelle commands and
conditional scaffolding still use their existing external/per-file write paths;
R05/R06 recovery does not yet cover them. Normal child builds still inherit stdio.

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
