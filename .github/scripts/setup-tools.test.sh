#!/usr/bin/env bash
# Offline regressions for the installer trust boundary.
set -euo pipefail

script_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
fixture=$(mktemp -d "${TMPDIR:-/tmp}/rebaze-rust-bootstrap-test.XXXXXXXX")
trap 'rm -r "$fixture"' EXIT
export REBAZE_BOOTSTRAP_TEST_LOG="$fixture/calls"
export CARGO_HOME="$fixture/cargo" RUSTUP_HOME="$fixture/rustup"
export RUNNER_TEMP="$fixture" GITHUB_PATH="$fixture/github-path" GITHUB_ENV="$fixture/github-env"
export REBAZE_BOOTSTRAP_TEST_MARKER="$fixture/installer-executed"
export REBAZE_BOOTSTRAP_REAL_SHA
REBAZE_BOOTSTRAP_REAL_SHA=$(command -v sha256sum || true)
mkdir -p "$fixture/bin"
# A controlled PATH prevents an installed rustup from masking bootstrap coverage.
for tool in bash mktemp rm chmod dirname; do
  ln -s "$(command -v "$tool")" "$fixture/bin/$tool"
done

curl() {
  printf 'curl\n' >>"$REBAZE_BOOTSTRAP_TEST_LOG"
  if [[ "${REBAZE_BOOTSTRAP_DOWNLOAD_FAIL:-0}" == 1 ]]; then return 22; fi
  while [[ "$1" != --output ]]; do shift; done
  # shellcheck disable=SC2016 # The downloaded fixture expands the marker when executed.
  printf '#!/bin/sh\n/usr/bin/touch "$REBAZE_BOOTSTRAP_TEST_MARKER"\n' >"$2"
}
uname() {
  case "$1" in
  -s) printf 'Linux\n' ;;
  -m) printf '%s\n' "${REBAZE_BOOTSTRAP_ARCH:-aarch64}" ;;
  esac
}
sha256sum() {
  # Compare the actual downloaded bytes with the committed expected digest.
  if [[ -n "$REBAZE_BOOTSTRAP_REAL_SHA" ]]; then
    "$REBAZE_BOOTSTRAP_REAL_SHA" "$@"
  else
    /usr/bin/shasum -a 256 "$@"
  fi
}
export -f curl uname sha256sum

for tool in rust bazel cmake jdk gradle bun; do
  test_arch=aarch64
  if [[ "$tool" == bun ]]; then test_arch=x86_64; fi
  if REBAZE_BOOTSTRAP_ARCH="$test_arch" PATH="$fixture/bin" bash "$script_dir/setup-$tool.sh" >"$fixture/bad-digest.log" 2>&1; then
    echo 'An invalid installer digest was accepted.' >&2
    exit 1
  fi
  grep -q 'checksum mismatch' "$fixture/bad-digest.log"
  test ! -e "$GITHUB_ENV"
  test ! -e "$GITHUB_PATH"
  if [[ -e "$REBAZE_BOOTSTRAP_TEST_MARKER" ]]; then
    echo "Invalid installer executed." >&2
    exit 1
  fi

  if REBAZE_BOOTSTRAP_DOWNLOAD_FAIL=1 REBAZE_BOOTSTRAP_ARCH="$test_arch" PATH="$fixture/bin" bash "$script_dir/setup-$tool.sh" >"$fixture/download.log" 2>&1; then
    echo 'A failed download was accepted.' >&2
    exit 1
  fi
  if [[ -e "$REBAZE_BOOTSTRAP_TEST_MARKER" ]]; then
    echo "Installer ran after failed download." >&2
    exit 1
  fi

done

: >"$REBAZE_BOOTSTRAP_TEST_LOG"
if REBAZE_BOOTSTRAP_ARCH=unsupported PATH="$fixture/bin" bash "$script_dir/setup-rust.sh" >"$fixture/architecture.log" 2>&1; then
  echo 'An unsupported architecture was accepted.' >&2
  exit 1
fi
test ! -s "$REBAZE_BOOTSTRAP_TEST_LOG"

rustup() {
  [[ "$TMPDIR" == "$RUNNER_TEMP" ]] || return 1
  printf '%s\n' "$*" >>"$REBAZE_BOOTSTRAP_TEST_LOG"
}
export -f rustup
PATH="$fixture/bin" bash "$script_dir/setup-rust.sh"
grep -qx 'toolchain install --no-self-update' "$REBAZE_BOOTSTRAP_TEST_LOG"
grep -qx "$CARGO_HOME/bin" "$GITHUB_PATH"
grep -qx "RUSTUP_HOME=$RUNNER_TEMP/rustup" "$GITHUB_ENV"
grep -qx "TMPDIR=$RUNNER_TEMP" "$GITHUB_ENV"
printf 'Tool bootstrap failure and existing-toolchain checks passed.\n'
