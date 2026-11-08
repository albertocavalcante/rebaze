#!/usr/bin/env bash
# Explicit networked oracle. Missing tools/checked-in fixtures are failures.
set -euo pipefail
script_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
repo=$(cd "$script_dir/../.." && pwd)
cli_dir=$(cd -- "$(dirname -- "${1:?Pass the built rebaze CLI path}")" && pwd)
cli="$cli_dir/$(basename -- "$1")"
test -x "$cli"
for tool in bazel cmake ctest make cc; do command -v "$tool" >/dev/null; done
real_bazel=$(command -v bazel)
work=$(mktemp -d "${TMPDIR:-/tmp}/rebaze-cmake-migration.XXXXXXXX")
cleanup() {
  # Bazel's extracted directories can be read-only; do not follow output symlinks.
  find "$work" -type d -exec chmod u+w {} +
  rm -r "$work"
}
trap cleanup EXIT
mkdir -p "$work/original/include" "$work/migrated/include" \
  "$work/original/tests" "$work/migrated/tests" "$work/bin" "$work/home"
fixture="$repo/crates/rebaze-cli/tests/fixtures/cmake"
files=(CMakeLists.txt dep.c main.c include/dep.h tests/CMakeLists.txt tests/scope.c)
for file in "${files[@]}"; do
  test -s "$fixture/$file"
  cp "$fixture/$file" "$work/original/$file"
  cp "$fixture/$file" "$work/migrated/$file"
done
ln -s "$script_dir/bazel-fixture-wrapper.sh" "$work/bin/bazel"
isolated() {
  env -i PATH="$work/bin:$PATH" HOME="$work/home" TMPDIR="${TMPDIR:-/tmp}" \
    REBAZE_FIXTURE_BAZEL="$real_bazel" \
    REBAZE_FIXTURE_CACHE="${REBAZE_CMAKE_TEST_CACHE:-$work/cache}" \
    BAZELISK_HOME="${BAZELISK_HOME:-$work/bazelisk}" USE_BAZEL_VERSION=9.0.0 "$@"
}
cmake --version
ctest --version
isolated bazel --version
cmake -S "$work/original" -B "$work/original-build"
cmake --build "$work/original-build" --parallel 2
ctest --test-dir "$work/original-build" --output-on-failure --no-tests=error
original_output=$("$work/original-build/probe" "two words'\$" 42 "")
test "$original_output" = 'rebaze fixture: 42'

# Unsupported configuration must fail before even original-build state is created.
printf '[filters]\nexcluded_define_patterns = ["CUSTOM"]\n' >"$work/migrated/rebaze.toml"
if isolated "$cli" migrate "$work/migrated" --build-generator native >"$work/config-rejection.log" 2>&1; then
  echo 'Unsupported configuration was silently ignored' >&2
  exit 1
fi
grep -q 'filters.excluded_define_patterns overrides are not implemented' "$work/config-rejection.log"
test ! -e "$work/migrated/.rebaze"
test ! -e "$work/migrated/BUILD.bazel"
rm "$work/migrated/rebaze.toml"

# Auto preview may inspect a tool candidate, but must not execute its version
# probe or build an external generator. The tripwire fails if it is executed.
ln -s "$script_dir/bazelle-preview-tripwire.sh" "$work/bin/bazelle"
isolated env REBAZE_PREVIEW_PROBE_MARKER="$work/preview-probe-called" \
  "$cli" migrate "$work/migrated" --dry-run >"$work/auto-preview.log"
test ! -e "$work/preview-probe-called"
grep -q 'Planned process' "$work/auto-preview.log"
test ! -e "$work/migrated/.rebaze"
test ! -e "$work/migrated/BUILD.bazel"
rm "$work/bin/bazelle"

# Static migration must reject scoped bodies before configuring the original project.
cp "$work/migrated/CMakeLists.txt" "$work/original-listfile"
printf '\nif(OFF)\nadd_library(hidden hidden.c)\nendif()\n' >>"$work/migrated/CMakeLists.txt"
if isolated "$cli" migrate "$work/migrated" --build-generator native >"$work/source-rejection.log" 2>&1; then
  echo 'Conditional targets were silently treated as unconditional' >&2
  exit 1
fi
grep -q 'requires CMake evaluation' "$work/source-rejection.log"
test ! -e "$work/migrated/.rebaze"
test ! -e "$work/migrated/BUILD.bazel"
cp "$work/original-listfile" "$work/migrated/CMakeLists.txt"

# Preview must not create either generated outputs or original-build state.
isolated "$cli" migrate "$work/migrated" --build-generator native --dry-run >"$work/preview.log"
test ! -e "$work/migrated/BUILD.bazel"
test ! -e "$work/migrated/.rebaze"
isolated "$cli" migrate "$work/migrated" --build-generator native
for file in "${files[@]}"; do cmp "$fixture/$file" "$work/migrated/$file"; done
(
  cd "$work/migrated"
  isolated bazel test //... --jobs=2 --test_output=errors --test_summary=detailed
)
migrated_output=$("$work/migrated/bazel-bin/probe" "two words'\$" 42 "")
test "$migrated_output" = "$original_output"

# Unsupported planning must preserve an existing generated BUILD.
generated_files=(BUILD.bazel MODULE.bazel .bazelversion .bazelrc)
mkdir "$work/generated-before"
for file in "${generated_files[@]}"; do
  cp "$work/migrated/$file" "$work/generated-before/$file"
done
assert_rejected() {
  local diagnostic=$1
  if isolated "$cli" migrate "$work/migrated" --build-generator native --unsafe-mode >"$work/rejection.log" 2>&1; then
    echo "Unsupported migration intent was accepted: $diagnostic" >&2
    exit 1
  fi
  if ! grep -q "$diagnostic" "$work/rejection.log"; then
    cat "$work/rejection.log" >&2
    exit 1
  fi
  for file in "${generated_files[@]}"; do
    cmp "$work/generated-before/$file" "$work/migrated/$file"
  done
  cp "$fixture/CMakeLists.txt" "$work/migrated/CMakeLists.txt"
}
# Launcher state must survive merging tests registered in a child directory.
printf '\nset_property(TARGET probe PROPERTY TEST_LAUNCHER some_launcher)\n' >>"$work/migrated/CMakeLists.txt"
assert_rejected 'set_property requires CMake evaluation'
# shellcheck disable=SC2016 # Preserve CMake's variable reference for the regression.
printf '\nset(PROP TEST_LAUNCHER)\nset_property(TARGET probe PROPERTY ${PROP} some_launcher)\n' >>"$work/migrated/CMakeLists.txt"
assert_rejected 'set_property requires CMake evaluation'
mkdir "$work/migrated/inactive"
printf 'add_test(NAME inactive COMMAND probe)\n' >"$work/migrated/inactive/CMakeLists.txt"
printf '\nif(OFF)\nadd_subdirectory(inactive)\nendif()\n' >>"$work/migrated/CMakeLists.txt"
assert_rejected 'conditional, loop, function'
rm -r "$work/migrated/inactive"

# Unsupported evaluation must also preserve every existing generated output.
for source in \
  'function(make_target)\nadd_library(hidden hidden.c)\nendfunction()' \
  'foreach(item a b)\nadd_library(hidden hidden.c)\nendforeach()' \
  'include(extra.cmake)' \
  'find_package(Toggle REQUIRED)' \
  'set(CMAKE_PROJECT_TOP_LEVEL_INCLUDES hook.cmake)' \
  'list(APPEND CMAKE_PROJECT_rebaze_fixture_INCLUDE hook.cmake)' \
  'list(REMOVE_ITEM DEP_SOURCES dep.c)' \
  'set(DEP_SOURCES dep.c CACHE STRING sources)' \
  'add_subdirectory(missing_required_directory)'; do
  printf '\n%b\n' "$source" >>"$work/migrated/CMakeLists.txt"
  if [[ "$source" == add_subdirectory* ]]; then
    assert_rejected 'missing_required_directory'
  else
    assert_rejected 'requires CMake evaluation'
  fi
done

# Unknown required links must fail before replacing the existing generated BUILD.
for dependency in Unknown::Required unknown_required -lunknown_required; do
  printf '\ntarget_link_libraries(probe %s)\n' "$dependency" >>"$work/migrated/CMakeLists.txt"
  assert_rejected 'target "probe" requires unresolved dependency'
done
printf '\nadd_library(dep-one STATIC dep.c)\nadd_library(dep_one STATIC dep.c)\n' >>"$work/migrated/CMakeLists.txt"
assert_rejected 'collides with'
printf '\ntarget_link_libraries(probe Unknown::dep)\n' >>"$work/migrated/CMakeLists.txt"
assert_rejected 'requires unresolved dependency'
printf '\nadd_library(Alias::one ALIAS Alias::two)\nadd_library(Alias::two ALIAS Alias::one)\n' >>"$work/migrated/CMakeLists.txt"
assert_rejected 'alias cycle'

for dependency in pkgconfig::undeclared PKGCONFIG::undeclared; do
  printf '\ntarget_link_libraries(probe %s)\n' "$dependency" >>"$work/migrated/CMakeLists.txt"
  assert_rejected 'requires unresolved dependency'
done
printf '\npkg_check_modules(Z REQUIRED IMPORTED_TARGET zlib)\nadd_library(PkgConfig::Z ALIAS dep)\n' >>"$work/migrated/CMakeLists.txt"
assert_rejected 'collides with a prepared pkg-config link'
printf '\nadd_library(Alias::dup ALIAS dep)\nadd_library(Alias::dup ALIAS dep)\n' >>"$work/migrated/CMakeLists.txt"
assert_rejected 'declared more than once'

# A successful build must not hide an actual assertion failure in either suite.
for root in original migrated; do
  printf '#include "dep.h"\nint dep_value(void) { return 43; }\n' >"$work/$root/dep.c"
done
cmake --build "$work/original-build" --parallel 2
status=0
ctest --test-dir "$work/original-build" --output-on-failure --no-tests=error || status=$?
test "$status" -eq 8
(
  cd "$work/migrated"
  isolated bazel build //... --jobs=2
  status=0
  isolated bazel test //... --jobs=2 --test_output=errors || status=$?
  test "$status" -eq 3
)
# A configured File API capture must describe the build just validated, not a
# stale reply from before the source changed. This path deliberately uses CMake
# evaluation (a conditional) and does not fall back to source extraction.
mkdir -p "$work/file-api" "$work/file-api-build/.cmake/api/v1/query"
touch "$work/file-api-build/.cmake/api/v1/query/codemodel-v2"
printf 'int main(void) { return 0; }\n' >"$work/file-api/main.c"
printf 'cmake_minimum_required(VERSION 3.20)\nproject(api C)\nadd_executable(before main.c)\n' >"$work/file-api/CMakeLists.txt"
cmake -S "$work/file-api" -B "$work/file-api-build"
printf 'cmake_minimum_required(VERSION 3.20)\nproject(api C)\nif(ON)\nadd_executable(after main.c)\nendif()\n' >"$work/file-api/CMakeLists.txt"
isolated "$cli" migrate "$work/file-api" --build-generator native \
  --cmake-build-dir "$work/file-api-build" --cmake-file-api-only
grep -q 'name = "after"' "$work/file-api/BUILD.bazel"
if grep -q 'name = "before"' "$work/file-api/BUILD.bazel"; then
  echo 'Migration used stale File API evidence' >&2
  exit 1
fi
# Configured data from another source root must not replace this project's files.
if isolated "$cli" migrate "$work/migrated" --build-generator native --unsafe-mode \
  --cmake-build-dir "$work/file-api-build" --cmake-file-api-only >"$work/foreign-api.log" 2>&1; then
  echo 'Foreign-project File API evidence was accepted' >&2
  exit 1
fi
grep -q 'does not match project root' "$work/foreign-api.log"
for file in "${generated_files[@]}"; do
  cmp "$work/generated-before/$file" "$work/migrated/$file"
done
printf 'Required CMake CLI migration passed: scope, temporal sources, refreshed File API, literal argv, test success and test failure.\n'
