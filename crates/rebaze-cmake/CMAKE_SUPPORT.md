# CMake Syntax Support

## File API Integration (Experimental)

rebaze can optionally read CMake's File API output from an existing build directory
to get a richer, build-system-accurate target graph (sources, includes, dependencies).

```bash
# After configuring with CMake
cmake -S . -B build

# Use the File API reply during migration
rebaze migrate . --from cmake --cmake-build-dir build

# Require File API and select a configuration
rebaze migrate . --from cmake --cmake-build-dir build --cmake-config Debug --cmake-file-api-only

# Skip pre/post build validation
rebaze migrate . --from cmake --unsafe-mode
```

## Parsing Status

### ✅ Fully Supported (Parsing)

| Feature | Example | Status |
|---------|---------|--------|
| Basic commands | `project(name)` | ✅ |
| Multi-line arguments | `add_executable(app\n  src1.cpp\n  src2.cpp)` | ✅ |
| Quoted strings | `"Hello World"` | ✅ |
| Bracket arguments | `[[raw content]]`, `[=[raw]=]` | ✅ |
| Variable references | `${VAR}`, `$ENV{VAR}`, `$CACHE{VAR}` | ✅ |
| Comments | `# comment`, `#[[ block ]]` | ✅ |
| Conditionals | `if()`, `elseif()`, `else()`, `endif()` | ✅ |
| Loops | `foreach()`, `endforeach()`, `while()`, `endwhile()` | ✅ |
| Functions/Macros | `function()`, `macro()`, `endfunction()`, `endmacro()` | ✅ |
| Simple generator exprs | `$<CONFIG:Debug>` | ✅ |
| Escape sequences | `\"`, `\\`, `\n`, `\t` | ✅ |

### ⚠️ Partial Support (Parsing)

| Feature | Example | Issue |
|---------|---------|-------|
| Complex generator exprs | `$<TARGET_FILE:target>` | Works, but no semantic understanding |

### ✅ Recently Added Support

| Feature | Example | Status |
|---------|---------|--------|
| Nested parentheses | `if(A AND (B OR C))` | ✅ Full support |
| Nested variable refs | `${${VAR}}` | ✅ Full support |
| Nested generator exprs | `$<$<CONFIG:Debug>:val>` | ✅ Full support |
| Quoted strings in parens | `if(NOT ("${x}" MATCHES "regex"))` | ✅ Full support |

## Semantic Support (Interpretation)

### ✅ Extracted for Bazel Migration

| Feature | CMake | Bazel Equivalent |
|---------|-------|------------------|
| `project()` | Project name, version | `module()` name |
| `add_executable()` | Binary targets | `cc_binary()` |
| `add_library(STATIC)` | Static libraries | `cc_library()` |
| `add_library(SHARED)` | Shared libraries | `cc_library(linkshared=True)` |
| `add_library(INTERFACE)` | Header-only | `cc_library(hdrs=...)` |
| `find_package()` | External deps | Mapped to Bazel deps |
| `target_link_libraries()` | Dependencies | `deps = [...]` |
| `target_include_directories()` | Include paths | `includes = [...]` |

### ❌ Not Interpreted

| Feature | Why |
|---------|-----|
| `set()` | Variables not tracked/expanded |
| `option()` | Build options not mapped |
| `if()/else()` | Conditional logic not evaluated |
| `foreach()` | Loops not unrolled |
| `file(GLOB)` | Glob patterns not expanded |
| `configure_file()` | Template processing not done |
| `execute_process()` | External commands not run |
| `include()` | Included files not parsed |
| `add_subdirectory()` | Subdirs listed but not recursively parsed |
| Generator expressions | Not evaluated for Bazel |

## Test Coverage

- **75/75** CMake files parse successfully (100%)
  - 40 files from ttroy50/cmake-examples
  - 35 files from nlohmann/json (complex real-world project)

## Priority Fixes Needed

1. **Variable expansion/list handling** - Needed for accurate source lists (set/foreach)
2. **Recursive include/add_subdirectory** - Required for multi-directory projects
3. **Generator expression evaluation** - Needed for accurate Bazel mapping
