# First Wave Refactoring Plan: File Decomposition

This document details the first wave of refactoring for rebaze, focusing on decomposing large files into well-organized modules.

## Target Files

| File | Lines | Priority | Complexity |
|------|-------|----------|------------|
| `rebaze-cmake/src/project.rs` | 1743 | 1 | High |
| `rebaze-bzlmod/src/graph/mod.rs` | 1473 | 2 | High |
| `rebaze-bzlmod/src/resolver/mod.rs` | 1374 | 3 | High |
| `rebaze-bazel/src/config.rs` | 1172 | 4 | Medium |
| `rebaze-bazel/src/third_party.rs` | 1015 | 5 | Medium |

---

## 1. `rebaze-cmake/src/project.rs` → `project/` Module

The largest file contains CMake project extraction logic with distinct responsibilities.

### Current Structure Analysis

```
project.rs (1743 lines)
├── Types (lines 1-126)
│   ├── ExtractError enum
│   ├── CMakeProject struct
│   ├── Executable struct
│   ├── Library struct + LibraryKind enum
│   ├── Package struct
│   └── PkgConfigModule struct
├── Public API (lines 128-221)
│   ├── extract_project()
│   ├── extract_project_with_context()
│   └── extract_project_from_path() + extract_recursive()
├── Command Extractors (lines 343-712)
│   ├── extract_cmake_version()
│   ├── extract_project_info()
│   ├── extract_executable()
│   ├── extract_library()
│   ├── extract_package()
│   ├── extract_subdirectory()
│   ├── extract_global_includes()
│   ├── extract_compile_features()
│   └── extract_pkg_config()
├── Target Property Applicators (lines 816-1134)
│   ├── apply_link_libraries()
│   ├── apply_include_directories()
│   ├── apply_compile_definitions()
│   ├── apply_compile_options()
│   └── apply_target_sources()
├── Path Utilities (lines 714-763)
│   ├── prefix_path()
│   └── normalize_include_path()
└── Tests (lines 1161-1743)
```

### Proposed Structure

```
rebaze-cmake/src/
├── project/
│   ├── mod.rs              # Re-exports + extract_project(), extract_project_from_path()
│   ├── types.rs            # ExtractError, CMakeProject, Executable, Library, Package, etc.
│   ├── extract.rs          # extract_project_with_context(), extract_recursive()
│   ├── commands/
│   │   ├── mod.rs          # dispatch_command() helper
│   │   ├── project_info.rs # extract_cmake_version(), extract_project_info()
│   │   ├── targets.rs      # extract_executable(), extract_library()
│   │   ├── dependencies.rs # extract_package(), extract_pkg_config()
│   │   └── subdirs.rs      # extract_subdirectory(), extract_global_includes()
│   ├── target_props.rs     # apply_* functions for target properties
│   ├── path_utils.rs       # prefix_path(), normalize_include_path()
│   └── tests.rs            # All tests (conditionally compiled)
└── lib.rs                  # Update to: pub mod project; pub use project::*;
```

### Implementation Steps

1. **Create `project/types.rs`** (~130 lines)
   - Move: `ExtractError`, `CMakeProject`, `Executable`, `Library`, `LibraryKind`, `Package`, `PkgConfigModule`
   - Add `#[derive(Debug, Clone)]` where missing

2. **Create `project/path_utils.rs`** (~50 lines)
   - Move: `prefix_path()`, `normalize_include_path()`
   - Mark as `pub(crate)` - internal to cmake crate

3. **Create `project/commands/mod.rs`** (~40 lines)
   - Dispatch enum or match helper for command routing
   - Re-export submodules

4. **Create `project/commands/project_info.rs`** (~100 lines)
   - Move: `extract_cmake_version()`, `extract_project_info()`, `is_project_keyword()`, `is_language()`

5. **Create `project/commands/targets.rs`** (~150 lines)
   - Move: `extract_executable()`, `extract_library()`, `extract_compile_features()`

6. **Create `project/commands/dependencies.rs`** (~100 lines)
   - Move: `extract_package()`, `extract_pkg_config()`

7. **Create `project/commands/subdirs.rs`** (~50 lines)
   - Move: `extract_subdirectory()`, `extract_global_includes()`

8. **Create `project/target_props.rs`** (~320 lines)
   - Move all `apply_*` functions

9. **Create `project/extract.rs`** (~250 lines)
   - Move: `extract_project_with_context()`, `extract_recursive()`
   - Keep `extract_project()` wrapper in `mod.rs`

10. **Create `project/mod.rs`** (~60 lines)
    - Re-export public types from `types.rs`
    - Keep `extract_project()` and `extract_project_from_path()` as public API
    - Internal module declarations

---

## 2. `rebaze-bzlmod/src/graph/mod.rs` → `graph/` Submodules

### Current Structure Analysis

```
graph/mod.rs (1473 lines)
├── Core Graph Struct (lines 1-90)
│   └── DependencyGraph with fields
├── Basic Operations (lines 91-170)
│   ├── new(), with_root(), set_root(), root()
│   ├── add_module(), add_edge()
│   ├── record_request(), record_override()
│   └── get(), get_by_name(), contains(), len(), is_empty()
├── Traversal (lines 171-302)
│   ├── direct_deps(), direct_dep_keys()
│   ├── reverse_deps(), reverse_dep_keys()
│   ├── transitive_deps(), transitive_dependents()
│   ├── path(), all_paths(), find_all_paths_dfs()
├── Explanation System (lines 304-469)
│   ├── explain()
│   ├── build_selection_info()
│   ├── build_reason_string()
│   └── why_included()
├── Export (lines 471-590)
│   ├── to_dot()
│   ├── to_json()
│   ├── to_bazel_format(), build_bazel_deps()
├── Cycle Detection (lines 592-681)
│   ├── has_cycles(), has_cycle_dfs()
│   └── find_cycles(), find_cycles_dfs()
├── Stats (lines 683-765)
│   ├── roots(), leaves()
│   ├── stats(), calculate_max_depth()
├── Text Output (lines 767-860)
│   └── to_text(), print_tree(), format_tree_line()
├── Types (lines ~900-1063)
│   ├── Explanation, RequesterInfo, DependencyChain
│   ├── SelectionInfo, SelectionStrategy, VersionCandidate
│   ├── GraphStats
│   └── BazelModGraph, BazelDependency (Bazel JSON format)
└── Tests (lines 1065-1473)
```

### Proposed Structure

```
rebaze-bzlmod/src/graph/
├── mod.rs              # DependencyGraph struct + basic ops + re-exports
├── types.rs            # Explanation types, GraphStats, Bazel format types
├── traversal.rs        # path(), all_paths(), transitive_deps(), BFS/DFS helpers
├── explain.rs          # explain(), why_included(), selection info
├── cycles.rs           # has_cycles(), find_cycles(), DFS helpers
├── export/
│   ├── mod.rs
│   ├── dot.rs          # to_dot()
│   ├── json.rs         # to_json(), to_bazel_format()
│   └── text.rs         # to_text(), print_tree()
└── tests.rs            # All tests
```

### Implementation Steps

1. **Create `graph/types.rs`** (~170 lines)
   - Move: `Explanation`, `RequesterInfo`, `DependencyChain`, `SelectionInfo`, `SelectionStrategy`, `VersionCandidate`, `GraphStats`, `BazelModGraph`, `BazelDependency`

2. **Create `graph/traversal.rs`** (~200 lines)
   - Move: `transitive_deps()`, `transitive_dependents()`, `path()`, `all_paths()`, `find_all_paths_dfs()`

3. **Create `graph/explain.rs`** (~200 lines)
   - Move: `explain()`, `why_included()`, `build_selection_info()`, `build_reason_string()`

4. **Create `graph/cycles.rs`** (~100 lines)
   - Move: `has_cycles()`, `has_cycle_dfs()`, `find_cycles()`, `find_cycles_dfs()`

5. **Create `graph/export/dot.rs`** (~50 lines)
   - Move: `to_dot()`

6. **Create `graph/export/json.rs`** (~80 lines)
   - Move: `to_json()`, `to_bazel_format()`, `build_bazel_deps()`

7. **Create `graph/export/text.rs`** (~100 lines)
   - Move: `to_text()`, `print_tree()`, `format_tree_line()`

8. **Refactor `graph/mod.rs`** (~300 lines)
   - Keep: `DependencyGraph` struct definition
   - Keep: Basic operations (new, add, get, contains, len)
   - Keep: Simple accessors (direct_deps, reverse_deps, roots, leaves)
   - Import and delegate to submodules

---

## 3. `rebaze-bzlmod/src/resolver/mod.rs` → `resolver/` Submodules

### Current Structure Analysis

```
resolver/mod.rs (1374 lines)
├── Constants (lines 1-38)
│   └── DEFAULT_MAX_CONCURRENCY, DEFAULT_MAX_DEPTH, DEFAULT_TIMEOUT_SECS
├── Types (lines 39-215)
│   ├── ResolverOptions + Default impl
│   ├── YankedBehavior, DirectDepsMode enums
│   ├── ResolutionResult, ResolutionStats
│   ├── DepRequest, GraphBuildContext
│   ├── DepTask
│   └── Resolver struct
├── Resolver impl (lines 216-1333)
│   ├── new(), add_override_module_content/info()
│   ├── resolve_dependencies() - main entry point
│   ├── build_dependency_graph() - async BFS with worker pool
│   ├── resolve_transitive_deps()
│   ├── process_deps()
│   ├── apply_overrides()
│   ├── select_mvs() - MVS algorithm
│   ├── build_module_graph()
│   ├── resolve_static() - convenience wrapper
│   └── compare_versions()
└── Tests (lines 1334-1374)
```

### Proposed Structure

```
rebaze-bzlmod/src/resolver/
├── mod.rs              # Resolver struct + resolve_dependencies() + re-exports
├── options.rs          # ResolverOptions, YankedBehavior, DirectDepsMode
├── result.rs           # ResolutionResult, ResolutionStats
├── context.rs          # GraphBuildContext, DepRequest, DepTask
├── graph_builder.rs    # build_dependency_graph(), resolve_transitive_deps(), process_deps()
├── overrides.rs        # apply_overrides(), add_override_module_*
├── mvs.rs              # select_mvs(), build_module_graph()
├── version.rs          # (already exists) - compare_versions already here
└── tests.rs            # All tests
```

### Implementation Steps

1. **Create `resolver/options.rs`** (~80 lines)
   - Move: `ResolverOptions`, `YankedBehavior`, `DirectDepsMode`, Default impl

2. **Create `resolver/result.rs`** (~50 lines)
   - Move: `ResolutionResult`, `ResolutionStats`

3. **Create `resolver/context.rs`** (~100 lines)
   - Move: `GraphBuildContext`, `DepRequest`, `DepTask`, GraphBuildContext impl

4. **Create `resolver/graph_builder.rs`** (~350 lines)
   - Move: `build_dependency_graph()`, `resolve_transitive_deps()`, `process_deps()`

5. **Create `resolver/overrides.rs`** (~120 lines)
   - Move: `apply_overrides()`, `add_override_module_content()`, `add_override_module_info()`

6. **Create `resolver/mvs.rs`** (~200 lines)
   - Move: `select_mvs()`, `build_module_graph()`, `resolve_static()`

7. **Refactor `resolver/mod.rs`** (~200 lines)
   - Keep: `Resolver` struct, `new()`, `resolve_dependencies()` orchestration
   - Re-export all public types

---

## 4. `rebaze-bazel/src/config.rs` → `config/` Module

### Current Structure Analysis

```
config.rs (1172 lines)
├── MigrationConfig struct (lines 15-33)
├── MigrationConfig impl (lines 38-286)
│   ├── load_from_file(), load_from_project()
│   ├── map_system_library()
│   ├── map_imported_target(), map_imported_target_full()
│   ├── map_dependency()
│   ├── is_ignored_library()
│   ├── map_plain_library()
│   ├── get_transitive_deps()
│   ├── get_system_linkopts()
│   └── get_known_package()
├── VersionConfig (lines 288-309)
├── MappingConfig (lines 311-677)
│   ├── struct definition
│   ├── Default impl
│   └── default_* methods (7 large functions)
├── Other types (lines 679-951)
│   ├── PackageMapping, package_mapping()
│   ├── KnownPackageInfo
│   ├── FilterConfig + Default
│   ├── BuildConfig + Default
│   └── StrategyConfig + Default
└── Tests (lines 952-1172)
```

### Proposed Structure

```
rebaze-bazel/src/
├── config/
│   ├── mod.rs              # MigrationConfig struct + load methods + re-exports
│   ├── types.rs            # VersionConfig, FilterConfig, BuildConfig, StrategyConfig
│   ├── mapping.rs          # MappingConfig struct, map_* methods
│   ├── mapping_defaults.rs # default_system_libraries, default_packages, etc.
│   ├── package_info.rs     # PackageMapping, KnownPackageInfo, package_mapping()
│   └── tests.rs            # All tests
└── lib.rs                  # Update: pub mod config; pub use config::MigrationConfig;
```

### Implementation Steps

1. **Create `config/types.rs`** (~200 lines)
   - Move: `VersionConfig`, `FilterConfig`, `BuildConfig`, `StrategyConfig` with their Default impls

2. **Create `config/package_info.rs`** (~80 lines)
   - Move: `PackageMapping`, `KnownPackageInfo`, `package_mapping()`

3. **Create `config/mapping_defaults.rs`** (~350 lines)
   - Move all `default_*` methods as standalone functions
   - Refactor: `MappingConfig::default_system_libraries()` → `default_system_libraries()`

4. **Create `config/mapping.rs`** (~250 lines)
   - Move: `MappingConfig` struct definition
   - Move: All `map_*` methods from `MigrationConfig` that relate to mapping
   - Keep `MappingConfig::default()` calling the defaults module

5. **Refactor `config/mod.rs`** (~200 lines)
   - Keep: `MigrationConfig` struct
   - Keep: `load_from_file()`, `load_from_project()`
   - Delegate mapping methods to `mapping.rs` module

---

## 5. `rebaze-bazel/src/third_party.rs` → `third_party/` Module

### Current Structure Analysis

```
third_party.rs (1015 lines)
├── deduplicate_modules() (lines 18-32)
├── DepsStrategy enum (lines 32-58)
├── ThirdPartyConfig struct (lines 59-69)
├── generate_build_file() (lines 71-183)
├── generate_config_bzl() (lines 185-290)
├── get_known_package_info() (lines 292-362)
├── KnownPackageInfo struct (lines 362-378)
├── generate_source_bzl() (lines 380-538)
├── generate_source_build_targets() (lines 540-632)
├── SystemLibInfo struct (lines 636-649)
├── probe_system_lib() (lines 651-730)
├── generate_system_deps_bzl() (lines 732-907)
├── PkgConfigFlags struct (lines 911-916)
├── probe_pkg_config() (lines 918-958)
├── generate_system_linkopts() (lines 960-996)
└── Tests (lines 997-1015)
```

### Proposed Structure

```
rebaze-bazel/src/
├── third_party/
│   ├── mod.rs              # ThirdPartyConfig, DepsStrategy, deduplicate_modules, re-exports
│   ├── build_file.rs       # generate_build_file()
│   ├── config_bzl.rs       # generate_config_bzl()
│   ├── source_bzl.rs       # generate_source_bzl(), generate_source_build_targets()
│   ├── system_deps.rs      # generate_system_deps_bzl(), generate_system_linkopts()
│   ├── pkg_config.rs       # probe_pkg_config(), probe_system_lib(), PkgConfigFlags, SystemLibInfo
│   ├── known_packages.rs   # get_known_package_info(), KnownPackageInfo
│   └── tests.rs            # All tests
└── lib.rs                  # Update: pub mod third_party; pub use third_party::*;
```

### Implementation Steps

1. **Create `third_party/pkg_config.rs`** (~150 lines)
   - Move: `PkgConfigFlags`, `SystemLibInfo`, `probe_pkg_config()`, `probe_system_lib()`

2. **Create `third_party/known_packages.rs`** (~100 lines)
   - Move: `KnownPackageInfo` struct, `get_known_package_info()`

3. **Create `third_party/build_file.rs`** (~120 lines)
   - Move: `generate_build_file()`

4. **Create `third_party/config_bzl.rs`** (~110 lines)
   - Move: `generate_config_bzl()`

5. **Create `third_party/source_bzl.rs`** (~250 lines)
   - Move: `generate_source_bzl()`, `generate_source_build_targets()`

6. **Create `third_party/system_deps.rs`** (~220 lines)
   - Move: `generate_system_deps_bzl()`, `generate_system_linkopts()`

7. **Refactor `third_party/mod.rs`** (~80 lines)
   - Keep: `ThirdPartyConfig`, `DepsStrategy`, `deduplicate_modules()`
   - Re-export public items from submodules

---

## Implementation Order

### Sprint 1: CMake Parser (Most Complex)
1. [ ] `rebaze-cmake/src/project/` decomposition
2. [ ] Add integration tests for project extraction

### Sprint 2: Bzlmod Graph
3. [ ] `rebaze-bzlmod/src/graph/` decomposition
4. [ ] Add unit tests for traversal/cycle detection

### Sprint 3: Bzlmod Resolver
5. [ ] `rebaze-bzlmod/src/resolver/` decomposition
6. [ ] Add tests for MVS algorithm edge cases

### Sprint 4: Bazel Generation
7. [ ] `rebaze-bazel/src/config/` decomposition
8. [ ] `rebaze-bazel/src/third_party/` decomposition
9. [ ] Add tests for config mapping

---

## Backward Compatibility

All decompositions maintain backward compatibility through:

1. **Re-exports in `mod.rs`** - All public types accessible from original module path
2. **Same function signatures** - No API changes
3. **Feature flags** - None required for first wave

Example from `rebaze-cmake/src/project/mod.rs`:
```rust
// Re-export all public types at module root
pub use self::types::{
    CMakeProject, Executable, ExtractError, Library, LibraryKind,
    Package, PkgConfigModule,
};

// Keep public API functions at module root
pub fn extract_project(file: &CMakeFile, path: PathBuf) -> CMakeProject {
    extract::extract_project(file, path)
}

pub fn extract_project_from_path(root: &Path) -> Result<CMakeProject, ExtractError> {
    extract::extract_project_from_path(root)
}
```

---

## Verification

After each decomposition:

```bash
# Ensure compilation
cargo check -p rebaze-cmake  # or relevant crate

# Run tests
cargo test -p rebaze-cmake

# Check for regressions in dependent crates
cargo test --workspace

# Verify no public API changes (semver check)
cargo semver-checks check-release -p rebaze-cmake
```
