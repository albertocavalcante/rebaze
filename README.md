# rebaze

Generate Bazel build files from CMake and Gradle projects, preview the result,
and validate the generated build.

rebaze is in early development. Review generated files before relying on them.
It targets Bazel 9; a successful parse or generation does not establish that
an arbitrary project builds correctly.

## Current scope

| Input           | Current behavior                                                                              |
| --------------- | --------------------------------------------------------------------------------------------- |
| CMake           | Native generation from parsed files or CMake File API data; optional bazelle generation       |
| Gradle          | Java/JUnit 4 generation from literal Groovy/Kotlin metadata; unsupported DSL fails explicitly |
| Maven           | Detection only; migration is unimplemented                                                    |
| Makefile, Cargo | Reported by `analyze`; migration is unimplemented                                             |

CMake test generation supports a small literal executable-backed subset. Custom
build logic, broader test semantics, dependency strategies, and platform
compatibility still have gaps. The [engineering roadmap](docs/engineering-roadmap.md)
records the evidence needed to close them.

## Build and try it

Use Bazelisk, or the Bazel version in [.bazelversion](.bazelversion), with a native
C/C++ toolchain. Bazel provisions the repository's Rust toolchain.

```bash
git clone https://github.com/albertocavalcante/rebaze.git
cd rebaze
bazel build //crates/rebaze-cli:rebaze
bazel run //crates/rebaze-cli:rebaze -- --help
```

Run these commands from the rebaze checkout, replacing `/path/to/project` with
an existing project directory:

```bash
bazel run //crates/rebaze-cli:rebaze -- analyze /path/to/project
bazel run //crates/rebaze-cli:rebaze -- migrate /path/to/project --build-generator native --dry-run
```

The dry run prints proposed files without writing them or running CMake/Bazel
build validation. After reviewing the proposal, run:

```bash
bazel run //crates/rebaze-cli:rebaze -- migrate /path/to/project --build-generator native
```

Migration can replace existing generated files. Use a clean branch or a copy of
the input project so you can inspect and revert the diff. CMake migration normally
configures and builds the original project first; both migrations then run
`bazel build //...`. These checks do not run the project's test suite. Use `rebaze validate` to repeat
the Bazel build after editing generated files.

`auto`, the default generator, prefers bazelle for CMake when available. Use
`--build-generator native` for the path shown above. `--unsafe-mode` skips build
validation; it does not establish migration correctness.

For a Cargo build, install the toolchain in [rust-toolchain.toml](rust-toolchain.toml)
and run `cargo build --release --locked -p rebaze-cli`.

## Documentation and development

- [User documentation](https://albertocavalcante.github.io/rebaze/): installation,
  tutorials, task guides, and CLI/configuration reference.
- [Contributing](CONTRIBUTING.md): setup, checks, tests, and pull requests.
- [Review guide](REVIEW.md): required review evidence and failure cases.
- [Architecture](docs/architecture.md): crate responsibilities and generation boundaries.
- [Engineering roadmap](docs/engineering-roadmap.md): ordered work and completion gates.
- [Documentation guide](docs/README.md): where each kind of documentation belongs.
- [CI runner operations](docs/operations/ci-runners.md): runmux activation and rollback.

## License

rebaze is licensed under the [Apache License 2.0](LICENSE). Release archives
include the license text.
