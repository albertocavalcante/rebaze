#!/usr/bin/env bash
# Explicit networked Gradle/JUnit4 -> Bazel oracle; required inputs never skip.
set -euo pipefail
script_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
repo=$(cd "$script_dir/../.." && pwd)
cli_dir=$(cd -- "$(dirname -- "${1:?Pass the built rebaze CLI path}")" && pwd)
cli="$cli_dir/$(basename -- "$1")"
test -x "$cli"
: "${JAVA_HOME:?Set JAVA_HOME to a JDK 21 installation}"
test -x "$JAVA_HOME/bin/javac"
for tool in bazel gradle; do command -v "$tool" >/dev/null; done
real_bazel=$(command -v bazel)
work=$(mktemp -d "${TMPDIR:-/tmp}/rebaze-java-migration.XXXXXXXX")
cleanup() {
  find "$work" -type d -exec chmod u+w {} +
  rm -r "$work"
}
trap cleanup EXIT
mkdir -p "$work/bin" "$work/home" "$work/original" "$work/migrated"
ln -s "$script_dir/bazel-fixture-wrapper.sh" "$work/bin/bazel"
fixture="$repo/crates/rebaze-cli/tests/fixtures/gradle"
files=(settings.gradle build.gradle core/build.gradle app/build.gradle
  core/src/main/java/example/core/Core.java app/src/main/java/example/app/Answer.java
  app/src/main/resources/value.txt app/src/test/resources/test-value.txt
  app/src/test/java/example/app/AnswerTest.java app/src/test/java/example/app/Behavior.java
  app/src/test/java/example/app/TestSupport.java maven_install.json gradle/verification-metadata.xml)
for file in "${files[@]}"; do
  test -s "$fixture/$file"
  for root in original migrated; do
    mkdir -p "$(dirname -- "$work/$root/$file")"
    cp "$fixture/$file" "$work/$root/$file"
  done
done
isolated() {
  env -i PATH="$work/bin:$JAVA_HOME/bin:$PATH" HOME="$work/home" JAVA_HOME="$JAVA_HOME" \
    TMPDIR="${TMPDIR:-/tmp}" GRADLE_USER_HOME="$work/gradle-home" \
    REBAZE_FIXTURE_BAZEL="$real_bazel" REBAZE_FIXTURE_CACHE="${REBAZE_JAVA_TEST_CACHE:-$work/cache}" \
    BAZELISK_HOME="${BAZELISK_HOME:-$work/bazelisk}" USE_BAZEL_VERSION=9.0.0 "$@"
}
reports() {
  local failures=$1 suite
  for class in AnswerTest Behavior; do
    suite=$(grep '<testsuite ' "$work/original/app/build/test-results/test/TEST-example.app.$class.xml")
    for attribute in 'tests="1"' 'skipped="0"' 'errors="0"' "failures=\"$failures\""; do
      [[ "$suite" == *"$attribute"* ]]
    done
  done
}
isolated java -version
isolated gradle --version
isolated bazel --version
(
  cd "$work/original"
  isolated gradle --no-daemon --max-workers=2 test
)
reports 0
isolated "$cli" migrate "$work/migrated" --build-generator native --dry-run >"$work/preview.log"
test ! -e "$work/migrated/BUILD.bazel"
isolated "$cli" migrate "$work/migrated" --build-generator native
for file in "${files[@]}"; do cmp "$fixture/$file" "$work/migrated/$file"; done
(
  cd "$work/migrated"
  targets=$(isolated bazel query 'kind("java_test rule", //...)' --output=label | LC_ALL=C sort)
  test "$targets" = $'//app:test_example_app_AnswerTest\n//app:test_example_app_Behavior'
  isolated bazel test //... --jobs=2 --test_output=errors
)
cp "$work/migrated/app/BUILD.bazel" "$work/app-before.bazel"
mv "$work/migrated/maven_install.json" "$work/held-maven.json"
if isolated "$cli" migrate "$work/migrated" --build-generator native >"$work/unpinned.log" 2>&1; then
  echo 'Unpinned Maven dependencies were built without explicit preparation.' >&2
  exit 1
fi
grep -q 'require a reviewed maven_install.json' "$work/unpinned.log"
cmp "$work/app-before.bazel" "$work/migrated/app/BUILD.bazel"
mv "$work/held-maven.json" "$work/migrated/maven_install.json"
assert_rejected() {
  local diagnostic=$1
  if isolated "$cli" migrate "$work/migrated" --build-generator native --unsafe-mode >"$work/rejection.log" 2>&1; then
    echo "Unsupported Java intent was accepted: $diagnostic" >&2
    exit 1
  fi
  if ! grep -q "$diagnostic" "$work/rejection.log"; then
    cat "$work/rejection.log" >&2
    exit 1
  fi
  cmp "$work/app-before.bazel" "$work/migrated/app/BUILD.bazel"
}
printf '\ntest { useJUnitPlatform() }\n' >>"$work/migrated/app/build.gradle"
assert_rejected 'unsupported Gradle block'
cp "$fixture/app/build.gradle" "$work/migrated/app/build.gradle"
printf 'package example.app; import org.junit.Test; public abstract class Behavior { @Test public void value() {} }\n' >"$work/migrated/app/src/test/java/example/app/Behavior.java"
assert_rejected 'abstract JUnit'
cp "$fixture/app/src/test/java/example/app/Behavior.java" "$work/migrated/app/src/test/java/example/app/Behavior.java"
mv "$work/migrated/app/src" "$work/held-src"
ln -s "$work/held-src" "$work/migrated/app/src"
assert_rejected 'source symlinks'
rm "$work/migrated/app/src"
mv "$work/held-src" "$work/migrated/app/src"
for root in original migrated; do printf '43\n' >"$work/$root/app/src/main/resources/value.txt"; done
(
  cd "$work/original"
  isolated gradle --no-daemon --max-workers=2 assemble testClasses
  result=0
  isolated gradle --no-daemon --max-workers=2 test || result=$?
  test "$result" -eq 1
)
reports 1
(
  cd "$work/migrated"
  isolated bazel build //... --jobs=2
  result=0
  isolated bazel test //... --jobs=2 --test_output=errors || result=$?
  test "$result" -eq 3
)
printf 'Required Java CLI migration passed: two JUnit classes, scopes, resources, and assertion failures.\n'
