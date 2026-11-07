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
files=(CMakeLists.txt dep.c main.c include/dep.h tests/CMakeLists.txt)
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

# Launcher state in a parent directory must not disappear when tests are merged.
cp "$work/migrated/BUILD.bazel" "$work/generated-before.bazel"
printf '\nset_property(TARGET probe PROPERTY TEST_LAUNCHER some_launcher)\n' >>"$work/migrated/CMakeLists.txt"
if isolated "$cli" migrate "$work/migrated" --build-generator native --unsafe-mode >"$work/unsupported.log" 2>&1; then
  echo 'Unsupported test launcher was silently accepted.' >&2
  exit 1
fi
grep -q 'launchers/emulators' "$work/unsupported.log"
cmp "$work/generated-before.bazel" "$work/migrated/BUILD.bazel"
cp "$fixture/CMakeLists.txt" "$work/migrated/CMakeLists.txt"

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
printf 'Required CMake CLI migration passed: builds, literal argv, test success and test failure.\n'
