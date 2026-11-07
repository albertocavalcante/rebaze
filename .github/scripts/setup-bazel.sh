#!/usr/bin/env bash
# Install the repository's pinned Bazel on minimal Linux runner images.
set -euo pipefail
script_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
# shellcheck source=.github/scripts/download-tool.sh
source "$script_dir/download-tool.sh"
prepare_job_temp
version=$(<"$script_dir/../../.bazelversion")
case "$version/$(uname -s)/$(uname -m)" in
9.0.0/Linux/aarch64)
  platform=linux-arm64
  digest=cab23c59d3d39c5e5382f12cd116b47445afdff9813516c18ae3ee8836b3037f
  ;;
9.0.0/Linux/x86_64)
  platform=linux-x86_64
  digest=c44a93f25398c68f904fa1d19b61d321de6c0d2f09dca375d7bc0dc9b9428403
  ;;
*)
  echo 'No verified Bazel digest for this version/platform; update the pins.' >&2
  exit 1
  ;;
esac
setup_dir=$(mktemp -d "${RUNNER_TEMP:-${TMPDIR:-/tmp}}/rebaze-bazel.XXXXXXXX")
trap 'rm -r "$setup_dir"' EXIT
download_verified_tool "https://github.com/bazelbuild/bazel/releases/download/$version/bazel-$version-$platform" "$digest" "$setup_dir/bazel"
tool_bin="${RUNNER_TEMP:-${TMPDIR:-/tmp}}/rebaze-tools/bin"
mkdir -p "$tool_bin"
mv "$setup_dir/bazel" "$tool_bin/bazel"
"$tool_bin/bazel" --version
if [[ -n "${GITHUB_PATH:-}" ]]; then printf '%s\n' "$tool_bin" >>"$GITHUB_PATH"; fi

publish_job_temp
