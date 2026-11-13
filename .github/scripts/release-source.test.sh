#!/usr/bin/env bash
# Offline regressions for release source resolution and verification.
set -euo pipefail

script_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
script="$script_dir/release-source.sh"
fixture=$(mktemp -d "${TMPDIR:-/tmp}/rebaze-release-source-test.XXXXXXXX")
trap 'rm -rf "$fixture"' EXIT

# Isolate from user and system configuration (signing, hooks, aliases).
export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1
export GIT_AUTHOR_NAME=test GIT_AUTHOR_EMAIL=test@example.invalid
export GIT_COMMITTER_NAME=test GIT_COMMITTER_EMAIL=test@example.invalid

failures=0
fail() {
  printf 'FAIL: %s\n' "$*" >&2
  failures=$((failures + 1))
}

git init --quiet --bare --initial-branch=main "$fixture/origin.git"
git init --quiet "$fixture/work"
git -C "$fixture/work" commit --quiet --allow-empty -m "first"
first=$(git -C "$fixture/work" rev-parse HEAD)
git -C "$fixture/work" tag v1.2.3
git -C "$fixture/work" tag -a v1.3.0-rc.1 -m "annotated prerelease"
git -C "$fixture/work" commit --quiet --allow-empty -m "second"
second=$(git -C "$fixture/work" rev-parse HEAD)
git -C "$fixture/work" remote add origin "$fixture/origin.git"
git -C "$fixture/work" push --quiet origin HEAD:refs/heads/main v1.2.3 v1.3.0-rc.1

# A shallow consumer clone, like actions/checkout with fetch-depth 1.
git clone --quiet --depth=1 --no-tags "file://$fixture/origin.git" "$fixture/clone"

# run_resolve OUTPUT_FILE VAR=VALUE... -> runs resolve in the clone; returns its status.
run_resolve() {
  local output=$1
  shift
  : >"$output"
  (cd "$fixture/clone" && env -u INPUT_TAG -u INPUT_PUBLISH -u REF_NAME -u REF_TYPE -u EVENT_NAME \
    GITHUB_OUTPUT="$output" "$@" bash "$script" resolve) >"$fixture/log" 2>&1
}

expect_output() {
  local output=$1 key=$2 want=$3 got
  got=$(sed -n "s/^$key=//p" "$output")
  [[ "$got" == "$want" ]] || fail "$key: want '$want', got '$got' ($(tr '\n' ' ' <"$fixture/log"))"
}

out="$fixture/out"

# Dispatch of a lightweight tag: exact commit, no publication by default.
if run_resolve "$out" EVENT_NAME=workflow_dispatch INPUT_TAG=v1.2.3; then
  expect_output "$out" tag v1.2.3
  expect_output "$out" sha "$first"
  expect_output "$out" publish false
  expect_output "$out" prerelease false
else
  fail "dispatch of an existing lightweight tag was rejected"
fi

# Explicit publication and an annotated prerelease tag peeled to its commit.
if run_resolve "$out" EVENT_NAME=workflow_dispatch INPUT_TAG=v1.3.0-rc.1 INPUT_PUBLISH=true; then
  expect_output "$out" sha "$first"
  expect_output "$out" publish true
  expect_output "$out" prerelease true
else
  fail "dispatch of an existing annotated tag was rejected"
fi

# Tag pushes always publish.
if run_resolve "$out" EVENT_NAME=push REF_TYPE=tag REF_NAME=v1.2.3; then
  expect_output "$out" sha "$first"
  expect_output "$out" publish true
else
  fail "a tag push was rejected"
fi

# Rejected inputs never write outputs.
reject() {
  local reason=$1
  shift
  if run_resolve "$out" "$@"; then
    fail "accepted: $reason"
  elif [[ -s "$out" ]]; then
    fail "wrote outputs for: $reason"
  fi
}
reject "a branch push" EVENT_NAME=push REF_TYPE=branch REF_NAME=main
reject "an unsupported event" EVENT_NAME=pull_request INPUT_TAG=v1.2.3
reject "a missing tag" EVENT_NAME=workflow_dispatch INPUT_TAG=v9.9.9
reject "an empty tag" EVENT_NAME=workflow_dispatch INPUT_TAG=
reject "a branch name as tag" EVENT_NAME=workflow_dispatch INPUT_TAG=main
reject "a two-part version" EVENT_NAME=workflow_dispatch INPUT_TAG=v1.2
reject "a version without v" EVENT_NAME=workflow_dispatch INPUT_TAG=1.2.3
# shellcheck disable=SC2016 # The literal $(...) must reach the script unexpanded.
reject "shell metacharacters" EVENT_NAME=workflow_dispatch 'INPUT_TAG=v1.2.3$(touch pwned)'
reject "an invalid publish flag" EVENT_NAME=workflow_dispatch INPUT_TAG=v1.2.3 INPUT_PUBLISH=yes
[[ ! -e "$fixture/clone/pwned" ]] || fail "shell metacharacters were executed"

# verify-head compares the checked-out commit with the resolved one.
git -C "$fixture/clone" fetch --quiet --depth=1 origin "+refs/tags/v1.2.3:refs/tags/v1.2.3"
git -C "$fixture/clone" checkout --quiet --detach v1.2.3
(cd "$fixture/clone" && bash "$script" verify-head "$first") >"$fixture/log" 2>&1 ||
  fail "verify-head rejected the matching commit"
if (cd "$fixture/clone" && bash "$script" verify-head "$second") >"$fixture/log" 2>&1; then
  fail "verify-head accepted a different commit"
fi
if (cd "$fixture/clone" && bash "$script" verify-head v1.2.3) >"$fixture/log" 2>&1; then
  fail "verify-head accepted a symbolic reference"
fi

# verify-remote-tag detects a tag moved after the build.
(cd "$fixture/clone" && bash "$script" verify-remote-tag v1.2.3 "$first") >"$fixture/log" 2>&1 ||
  fail "verify-remote-tag rejected an unchanged lightweight tag"
(cd "$fixture/clone" && bash "$script" verify-remote-tag v1.3.0-rc.1 "$first") >"$fixture/log" 2>&1 ||
  fail "verify-remote-tag rejected an unchanged annotated tag"
git -C "$fixture/work" tag --force v1.2.3 "$second"
git -C "$fixture/work" push --quiet --force origin v1.2.3
if (cd "$fixture/clone" && bash "$script" verify-remote-tag v1.2.3 "$first") >"$fixture/log" 2>&1; then
  fail "verify-remote-tag accepted a moved tag"
fi
git -C "$fixture/work" push --quiet origin :refs/tags/v1.3.0-rc.1
if (cd "$fixture/clone" && bash "$script" verify-remote-tag v1.3.0-rc.1 "$first") >"$fixture/log" 2>&1; then
  fail "verify-remote-tag accepted a deleted tag"
fi

if [[ "$failures" -ne 0 ]]; then
  printf '%d release source check(s) failed\n' "$failures" >&2
  exit 1
fi
echo "release source checks passed"
