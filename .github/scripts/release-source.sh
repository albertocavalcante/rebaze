#!/usr/bin/env bash
# Resolve and verify the exact source a release builds and publishes.
#
#   release-source.sh resolve
#       Select the release tag from the triggering event, validate it, fetch it
#       from origin and peel it to its commit. Writes tag, sha, publish and
#       prerelease to $GITHUB_OUTPUT. Inputs (environment): EVENT_NAME, REF_TYPE,
#       REF_NAME (tag pushes), INPUT_TAG and INPUT_PUBLISH (workflow_dispatch).
#   release-source.sh verify-head SHA
#       Fail unless the checked-out HEAD is exactly SHA.
#   release-source.sh verify-remote-tag TAG SHA
#       Fail unless TAG on origin still resolves (peeled) to SHA.
#
# Inputs are passed through the environment and arguments, never interpolated
# into shell source by the workflow.
set -euo pipefail

die() {
  printf '::error::%s\n' "$*" >&2
  exit 1
}

valid_tag() {
  [[ "$1" =~ ^v[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.]+)?$ ]]
}

valid_sha() {
  [[ "$1" =~ ^[0-9a-f]{40}$ ]]
}

resolve() {
  local tag publish sha prerelease
  case "${EVENT_NAME:-}" in
  workflow_dispatch)
    tag=${INPUT_TAG:-}
    case "${INPUT_PUBLISH:-false}" in
    true) publish=true ;;
    false | '') publish=false ;;
    *) die "publish input must be true or false" ;;
    esac
    ;;
  push)
    [[ "${REF_TYPE:-}" == tag ]] || die "release pushes must be tag pushes"
    tag=${REF_NAME:-}
    publish=true
    ;;
  *) die "unsupported release event: ${EVENT_NAME:-<unset>}" ;;
  esac

  valid_tag "$tag" || die "invalid release tag '$tag' (expected vMAJOR.MINOR.PATCH[-PRERELEASE])"
  git fetch --quiet --no-tags --depth=1 origin "+refs/tags/$tag:refs/tags/$tag" ||
    die "release tag $tag was not found on origin"
  sha=$(git rev-parse --verify --quiet "refs/tags/$tag^{commit}") ||
    die "release tag $tag does not resolve to a commit"

  prerelease=false
  if [[ "$tag" == *-* ]]; then
    prerelease=true
  fi

  {
    printf 'tag=%s\n' "$tag"
    printf 'sha=%s\n' "$sha"
    printf 'publish=%s\n' "$publish"
    printf 'prerelease=%s\n' "$prerelease"
  } >>"${GITHUB_OUTPUT:?GITHUB_OUTPUT must be set}"
  printf 'Release %s resolves to %s (publish=%s, prerelease=%s)\n' "$tag" "$sha" "$publish" "$prerelease"
}

verify_head() {
  local expected=$1 actual
  valid_sha "$expected" || die "expected release commit is not a full SHA: '$expected'"
  actual=$(git rev-parse --verify HEAD)
  [[ "$actual" == "$expected" ]] || die "checked out $actual but the release tag resolves to $expected"
  printf 'Checked-out source matches release commit %s\n' "$expected"
}

verify_remote_tag() {
  local tag=$1 expected=$2 refs peeled direct
  valid_tag "$tag" || die "invalid release tag '$tag'"
  valid_sha "$expected" || die "expected release commit is not a full SHA: '$expected'"
  refs=$(git ls-remote --tags origin "refs/tags/$tag" "refs/tags/$tag^{}") ||
    die "cannot list release tag $tag on origin"
  peeled=$(awk -v ref="refs/tags/$tag^{}" '$2 == ref { print $1 }' <<<"$refs")
  direct=$(awk -v ref="refs/tags/$tag" '$2 == ref { print $1 }' <<<"$refs")
  [[ -n "$direct" ]] || die "release tag $tag no longer exists on origin"
  [[ "${peeled:-$direct}" == "$expected" ]] ||
    die "release tag $tag now resolves to ${peeled:-$direct}, not the built commit $expected"
  printf 'Release tag %s still resolves to %s\n' "$tag" "$expected"
}

case "${1:-}" in
resolve)
  [[ $# -eq 1 ]] || die "usage: release-source.sh resolve"
  resolve
  ;;
verify-head)
  [[ $# -eq 2 ]] || die "usage: release-source.sh verify-head SHA"
  verify_head "$2"
  ;;
verify-remote-tag)
  [[ $# -eq 3 ]] || die "usage: release-source.sh verify-remote-tag TAG SHA"
  verify_remote_tag "$2" "$3"
  ;;
*) die "usage: release-source.sh resolve | verify-head SHA | verify-remote-tag TAG SHA" ;;
esac
