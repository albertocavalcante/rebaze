# Starlark Code Generation Refactoring Plan

## Current State Analysis

### What We Have

**`starlark.rs`** - Type-safe BUILD rule definitions:
- `Load`, `Package`, `Module`, `BazelDep` - work well with serde_starlark
- `CcLibrary`, `CcBinary` - used in `cmake_generator.rs`
- `Glob`, `SrcsWithHdrs`, `Add` - expression types
- `Alias`, `FunctionCall` - for aliases

**`third_party.rs`** - String templates (~700 lines):
- `generate_build_file()` - builds BUILD.bazel with aliases and cc_library
- `generate_config_bzl()` - generates Starlark functions (resolve_dep)
- `generate_source_bzl()` - generates module extension + SOURCES dict
- `generate_system_deps_bzl()` - generates repository_rule + module extension
- `generate_source_build_targets()` - cmake/meson/configure_make rules

### serde_starlark Limitations

serde_starlark is designed for **BUILD file rules** (data serialization), not full Starlark programs:

✅ **Can serialize:**
- Rule calls: `cc_library(name = "foo", srcs = [...])`
- Function calls: `load("...", "cc_library")`
- Data structures: lists, dicts, strings, booleans
- Expressions: `glob(["**/*.h"])`, `["a"] + ["b"]`

❌ **Cannot serialize:**
- Function definitions: `def foo():`
- Control flow: `if`, `for`, `continue`
- Variable assignments: `FOO = "bar"`
- Module extensions: `module_extension(implementation = _impl)`
- Repository rules: `repository_rule(implementation = _impl, attrs = {...})`

### Analysis by Function

| Function | Lines | String Templates | Can Use serde? |
|----------|-------|------------------|----------------|
| `generate_build_file` | ~60 | aliases, cc_library | ✅ Mostly yes |
| `generate_config_bzl` | ~100 | DEPS_CONFIG dict, resolve_dep function | ⚠️ Dict yes, function no |
| `generate_source_bzl` | ~140 | SOURCES dict, repository_rule, module_extension | ⚠️ Dict yes, rules no |
| `generate_system_deps_bzl` | ~120 | SYSTEM_LIBS dict, repository_rule, module_extension | ⚠️ Dict yes, rules no |
| `generate_source_build_targets` | ~120 | cmake, meson, configure_make rules | ✅ Yes |

## Refactoring Strategy

### Phase 1: Add Missing Rule Types (Low effort, high value)

Add to `starlark.rs`:
```rust
// rules_foreign_cc rules
#[derive(Serialize)]
#[serde(rename = "cmake")]
pub struct Cmake { ... }

#[derive(Serialize)]
#[serde(rename = "meson")]
pub struct Meson { ... }

#[derive(Serialize)]
#[serde(rename = "configure_make")]
pub struct ConfigureMake { ... }
```

**Impact:** Can replace ~120 lines in `generate_source_build_targets()`

### Phase 2: Refactor BUILD.bazel Generation (Medium effort)

Use existing types for `generate_build_file()`:
- Use `Alias` for dependency aliases
- Use `CcLibrary` for system library fallbacks
- Use `Load`, `Package` for headers

**Impact:** ~60 lines cleaner, type-safe

### Phase 3: Separate Data from Templates (Medium effort)

For .bzl files, separate concerns:
1. **Data structures** (can serialize): dicts, lists
2. **Code templates** (keep as strings): functions, control flow

```rust
// Data part - serialize with serde
#[derive(Serialize)]
struct SourceConfig {
    url: String,
    sha256: String,
    build_system: String,
    // ...
}

// Template part - keep as string
fn render_module_extension_template(data: &str) -> String {
    format!(r#"
SOURCES = {data}

def _source_deps_impl(module_ctx):
    for name, config in SOURCES.items():
        # ... template code
"#)
}
```

**Impact:** Data is type-safe, templates are minimal

### Phase 4: Known Packages Database (Future)

Move `KnownPackageInfo` to external config:
- TOML/JSON file for package metadata
- Extensible without code changes
- Community contributions possible

## Recommended Implementation Order

1. **Add `Cmake`, `Meson`, `ConfigureMake` types** (1 hour)
   - Immediate win for `generate_source_build_targets()`

2. **Refactor `generate_build_file()` to use types** (2 hours)
   - Use `Alias`, `CcLibrary`, `Load`, `Package`

3. **Create data types for .bzl dicts** (2 hours)
   - `SourceConfig`, `SystemLibConfig`
   - Serialize dicts, keep function templates

4. **External package database** (future)
   - Not urgent, but good for extensibility

## Example: Before and After

### Before (generate_source_build_targets)
```rust
match info.build_system {
    "cmake" => {
        lines.push(format!(
            r#"cmake(
    name = "{name}_source",
    lib_source = "@{name}_src//:all",
    out_static_libs = [{out_libs}],
    generate_args = ["-GNinja"],{deps}
    visibility = ["//visibility:public"],
)"#,
            name = info.name,
            out_libs = out_libs_str,
            deps = deps_str,
        ));
    }
    // ... more cases
}
```

### After
```rust
match info.build_system {
    "cmake" => {
        let rule = Cmake::new(
            format!("{}_source", info.name),
            format!("@{}_src//:all", info.name),
        )
        .with_static_libs(info.out_libs.clone())
        .with_deps(deps)
        .with_visibility_public();

        serde_starlark::to_string(&rule)?
    }
    // ... more cases
}
```

## Conclusion

**Intentionally kept as strings:**
- Starlark function definitions (resolve_dep, custom_resolve_dep)
- Module extension implementations (_system_deps_impl, _source_deps_impl)
- Repository rule implementations (_system_lib_repo_impl)

**Should use serde_starlark:**
- All BUILD rule calls (cc_library, cmake, meson, alias)
- Data dicts (SOURCES, SYSTEM_LIBS, DEPS_CONFIG)
- Load statements, package declarations

**Estimated total effort:** 5-6 hours for Phases 1-3
**Lines of code reduction:** ~200 lines of string templates
**Type safety improvement:** Compile-time checking for rule fields
