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
3. `rebaze-bazel` constructs a map of output paths to contents. Its generation
   APIs return `GenerationResult<HashMap<String, String>>`.
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

| Boundary           | Current contract and limit                                                                                                                                                          |
| ------------------ | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Parsing/extraction | Syntax functions consume source data; file-oriented entry points also read project files. CMake extraction supports a subset of CMake semantics.                                    |
| Configuration      | `MigrationConfig` owns versions, mappings, filters, custom rules, strategy settings, and the known source-package catalog. Some strategy settings are not consistently honored yet. |
| Host discovery     | Third-party generation still runs `pkg-config` inside `rebaze-bazel`. Host paths/flags therefore affect output. Making this an explicit snapshot is the next implementation slice.  |
| Starlark rendering | Models serialize values through `serde_starlark`. Validated identifiers, references, and calls distinguish generator syntax from project data.                                      |
| Output writing     | All paths are preflighted. Each file uses a temporary file and atomic replacement. This is not a transaction across the complete output set.                                        |
| Build validation   | Core executes CMake/Bazel and reports unsuccessful status. A failed post-generation build leaves generated files available for inspection.                                          |

## Starlark generation

`File` combines and hoists loads before rule calls while preserving body order.
Typed models cover native/JVM rules, extensions, module dependencies, collections,
and assignments. Invalid binding/call names or serialization errors propagate
through generation to core; they are not converted into successful output comments.

Fixed helper programs live in `crates/rebaze-bazel/src/templates/*.bzl`. Dynamic
configuration is assigned through serialized tables. The system helper uses
`json.encode` when it constructs a nested BUILD file from dynamic values.
Keep the boundary between trusted helper code and project data explicit.

One configurable `KnownPackageInfo` catalog supplies source archive metadata and
build targets. `SourceBuildSystem` distinguishes Meson, CMake, and Autotools.
Dependency traversal unions catalog dependencies with configured transitive
edges and terminates using a visited set. Termination does not establish that a
cyclic dependency graph is buildable by Bazel.

Source repositories require a 64-character hexadecimal SHA-256 and use Bazel's
checksum verification before extraction. Unknown packages need configuration;
source metadata and extension activation do not constitute universal build support.

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
end-to-end, strategy, resource-limit, and compatibility gates.
